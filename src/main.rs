use std::io::Cursor;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{Method, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use serde::{Deserialize, Serialize};
use sherpa_onnx::{
    OfflineParaformerModelConfig, OfflinePunctuation, OfflinePunctuationConfig, OfflineRecognizer,
    OfflineRecognizerConfig,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

const DEFAULT_MODEL_DIR: &str = "models/paraformer-zh-small-2024-03-09";
const DEFAULT_MODEL_FILE: &str = "model.int8.onnx";
const DEFAULT_TOKENS_FILE: &str = "tokens.txt";
const DEFAULT_PUNCT_MODEL_DIR_NAME: &str = "punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8";
const DEFAULT_PUNCT_MODEL_DIR: &str =
    "models/punct-ct-transformer-zh-en-vocab272727-2024-04-12-int8";
const DEFAULT_PUNCT_MODEL_FILE: &str = "model.int8.onnx";
const EMBEDDED_INDEX_HTML: &str = include_str!("../index.html");

#[derive(Parser, Debug)]
#[command(author, version, about = "Local ASR sidecar for VoiceBox")]
struct Args {
    #[arg(long, env = "VOICEBOX_HOST", default_value = "127.0.0.1")]
    host: IpAddr,

    #[arg(long, env = "VOICEBOX_PORT", default_value_t = 8765)]
    port: u16,

    #[arg(long, env = "VOICEBOX_MODEL")]
    model: Option<PathBuf>,

    #[arg(long, env = "VOICEBOX_TOKENS")]
    tokens: Option<PathBuf>,

    #[arg(long, env = "VOICEBOX_MODEL_DIR")]
    model_dir: Option<PathBuf>,

    #[arg(long, env = "VOICEBOX_PUNCT_MODEL")]
    punct_model: Option<PathBuf>,

    #[arg(long, env = "VOICEBOX_THREADS", default_value_t = 2)]
    threads: i32,

    #[arg(long, env = "VOICEBOX_LANGUAGE", default_value = "zh")]
    language: String,

    #[arg(long, env = "VOICEBOX_PROVIDER", default_value = "cpu")]
    provider: String,

    #[arg(long, env = "VOICEBOX_DEBUG", default_value_t = false)]
    debug: bool,
}

#[derive(Clone)]
struct AppState {
    engine: Arc<AsrEngine>,
}

struct AsrEngine {
    model_root: PathBuf,
    model_path: PathBuf,
    tokens_path: PathBuf,
    punctuation_model_path: Option<PathBuf>,
    default_language: String,
    provider: String,
    debug: bool,
    threads: i32,
    punctuator: Option<Mutex<OfflinePunctuation>>,
}

#[derive(Debug, Deserialize)]
struct TranscribeQuery {
    language: Option<String>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    ready: bool,
    model_family: String,
    model_root: String,
    model_path: String,
    tokens_path: String,
    punctuation_enabled: bool,
    punctuation_model_path: Option<String>,
    default_language: String,
    provider: String,
    threads: i32,
}

#[derive(Debug, Serialize)]
struct Segment {
    start_ms: i64,
    end_ms: i64,
    text: String,
}

#[derive(Debug, Serialize)]
struct TranscriptionResponse {
    ok: bool,
    text: String,
    language: String,
    elapsed_ms: u128,
    audio_duration_ms: u64,
    segments: Vec<Segment>,
}

#[derive(Debug)]
struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let payload = Json(serde_json::json!({
            "ok": false,
            "error": self.message,
        }));
        (self.status, payload).into_response()
    }
}

fn normalize_language(language: Option<&str>, fallback: &str) -> String {
    let candidate = language.unwrap_or(fallback).trim().to_lowercase();
    match candidate.as_str() {
        "" => fallback.to_owned(),
        "auto" | "zh" | "zh-cn" | "cmn" | "cmn-hans-cn" => "zh".to_owned(),
        _ => candidate,
    }
}

fn ensure_supported_language(language: &str) -> Result<(), AppError> {
    if language == "zh" {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "当前 79M Paraformer 小模型仅支持中文，请使用 language=zh。",
        ))
    }
}

fn dedupe_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut unique = Vec::new();
    for path in paths {
        if !unique.iter().any(|item| item == &path) {
            unique.push(path);
        }
    }
    unique
}

fn format_candidate_dirs(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn path_parent(path: &Path) -> Option<PathBuf> {
    path.parent().map(Path::to_path_buf)
}

fn candidate_model_dirs(args: &Args) -> Vec<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let cwd = std::env::current_dir().ok();

    dedupe_paths(
        args.model_dir
            .iter()
            .cloned()
            .chain(args.model.iter().filter_map(|path| path_parent(path)))
            .chain(args.tokens.iter().filter_map(|path| path_parent(path)))
            .chain(exe_dir.into_iter().map(|path| path.join(DEFAULT_MODEL_DIR)))
            .chain(cwd.into_iter().map(|path| path.join(DEFAULT_MODEL_DIR))),
    )
}

fn punct_model_candidate_dirs(args: &Args) -> Vec<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let cwd = std::env::current_dir().ok();

    dedupe_paths(
        args.punct_model
            .iter()
            .filter_map(|path| path_parent(path))
            .chain(
                args.model_dir
                    .iter()
                    .filter_map(|path| path_parent(path))
                    .map(|path| path.join(DEFAULT_PUNCT_MODEL_DIR_NAME)),
            )
            .chain(
                args.model
                    .iter()
                    .filter_map(|path| path_parent(path))
                    .filter_map(|path| path_parent(&path))
                    .map(|path| path.join(DEFAULT_PUNCT_MODEL_DIR_NAME)),
            )
            .chain(
                args.tokens
                    .iter()
                    .filter_map(|path| path_parent(path))
                    .filter_map(|path| path_parent(&path))
                    .map(|path| path.join(DEFAULT_PUNCT_MODEL_DIR_NAME)),
            )
            .chain(
                exe_dir
                    .into_iter()
                    .map(|path| path.join(DEFAULT_PUNCT_MODEL_DIR)),
            )
            .chain(
                cwd.into_iter()
                    .map(|path| path.join(DEFAULT_PUNCT_MODEL_DIR)),
            ),
    )
}

fn resolve_asset_path(
    explicit_path: Option<&PathBuf>,
    file_name: &str,
    candidate_dirs: &[PathBuf],
) -> Result<PathBuf, AppError> {
    if let Some(path) = explicit_path {
        if path.exists() {
            return Ok(path.clone());
        }

        return Err(AppError::bad_request(format!(
            "{file_name} not found: {}",
            path.display()
        )));
    }

    for dir in candidate_dirs {
        let candidate = dir.join(file_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(AppError::bad_request(format!(
        "Could not locate {file_name}. Searched: {}",
        format_candidate_dirs(candidate_dirs)
    )))
}

fn resolve_optional_asset_path(
    explicit_path: Option<&PathBuf>,
    file_name: &str,
    candidate_dirs: &[PathBuf],
) -> Result<Option<PathBuf>, AppError> {
    if let Some(path) = explicit_path {
        if path.exists() {
            return Ok(Some(path.clone()));
        }

        return Err(AppError::bad_request(format!(
            "{file_name} not found: {}",
            path.display()
        )));
    }

    for dir in candidate_dirs {
        let candidate = dir.join(file_name);
        if candidate.exists() {
            return Ok(Some(candidate));
        }
    }

    Ok(None)
}

impl AsrEngine {
    fn load(args: &Args) -> Result<Self, AppError> {
        let candidate_dirs = candidate_model_dirs(args);
        let model_path =
            resolve_asset_path(args.model.as_ref(), DEFAULT_MODEL_FILE, &candidate_dirs)?;
        let tokens_path =
            resolve_asset_path(args.tokens.as_ref(), DEFAULT_TOKENS_FILE, &candidate_dirs)?;
        let punct_candidate_dirs = punct_model_candidate_dirs(args);
        let punctuation_model_path = resolve_optional_asset_path(
            args.punct_model.as_ref(),
            DEFAULT_PUNCT_MODEL_FILE,
            &punct_candidate_dirs,
        )?;
        let model_root = model_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| AppError::internal("Resolved model path has no parent directory."))?;
        let punctuator = if let Some(path) = &punctuation_model_path {
            Some(Mutex::new(Self::create_punctuator(
                path,
                args.threads.max(1),
                args.provider.trim(),
                args.debug,
            )?))
        } else {
            None
        };

        let engine = Self {
            model_root,
            model_path,
            tokens_path,
            punctuation_model_path,
            default_language: normalize_language(Some(&args.language), "zh"),
            provider: args.provider.trim().to_owned(),
            debug: args.debug,
            threads: args.threads.max(1),
            punctuator,
        };

        ensure_supported_language(&engine.default_language)?;
        engine.validate()?;
        Ok(engine)
    }

    fn validate(&self) -> Result<(), AppError> {
        self.create_recognizer()?;
        Ok(())
    }

    fn create_recognizer(&self) -> Result<OfflineRecognizer, AppError> {
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.paraformer = OfflineParaformerModelConfig {
            model: Some(self.model_path.display().to_string()),
        };
        config.model_config.tokens = Some(self.tokens_path.display().to_string());
        config.model_config.provider = Some(self.provider.clone());
        config.model_config.debug = self.debug;
        config.model_config.num_threads = self.threads;

        OfflineRecognizer::create(&config).ok_or_else(|| {
            AppError::internal("Failed to create recognizer. Check model and tokens paths.")
        })
    }

    fn create_punctuator(
        model_path: &Path,
        threads: i32,
        provider: &str,
        debug: bool,
    ) -> Result<OfflinePunctuation, AppError> {
        let mut config = OfflinePunctuationConfig::default();
        config.model.ct_transformer = Some(model_path.display().to_string());
        config.model.num_threads = threads;
        config.model.provider = Some(provider.to_owned());
        config.model.debug = debug;

        OfflinePunctuation::create(&config).ok_or_else(|| {
            AppError::internal("Failed to create punctuator. Check punctuation model path.")
        })
    }

    fn restore_punctuation(&self, text: &str) -> Result<String, AppError> {
        let raw = text.trim();
        if raw.is_empty() {
            return Ok(String::new());
        }

        let Some(punctuator) = &self.punctuator else {
            return Ok(raw.to_owned());
        };

        let punctuator = punctuator
            .lock()
            .map_err(|_| AppError::internal("Punctuation model lock was poisoned."))?;

        match punctuator.add_punctuation(raw) {
            Some(text) => Ok(text.trim().to_owned()),
            None => {
                warn!("Punctuation restoration failed. Returning raw ASR text.");
                Ok(raw.to_owned())
            }
        }
    }

    fn transcribe(
        &self,
        wav_bytes: &[u8],
        language_override: Option<&str>,
    ) -> Result<TranscriptionResponse, AppError> {
        let audio = decode_wav(wav_bytes)?;
        if audio.samples.is_empty() {
            return Err(AppError::bad_request("Audio payload is empty."));
        }

        let language = normalize_language(language_override, &self.default_language);
        ensure_supported_language(&language)?;
        let recognizer = self.create_recognizer()?;
        let stream = recognizer.create_stream();

        let started_at = Instant::now();
        stream.accept_waveform(audio.sample_rate, &audio.samples);
        recognizer.decode(&stream);

        let result = stream
            .get_result()
            .ok_or_else(|| AppError::internal("Recognizer returned no transcription result."))?;
        let text = self.restore_punctuation(&result.text)?;

        Ok(TranscriptionResponse {
            ok: true,
            text,
            language,
            elapsed_ms: started_at.elapsed().as_millis(),
            audio_duration_ms: audio.duration_ms,
            segments: Vec::new(),
        })
    }
}

struct DecodedAudio {
    sample_rate: i32,
    samples: Vec<f32>,
    duration_ms: u64,
}

fn decode_wav(bytes: &[u8]) -> Result<DecodedAudio, AppError> {
    let cursor = Cursor::new(bytes);
    let mut reader = hound::WavReader::new(cursor)
        .map_err(|err| AppError::bad_request(format!("Invalid WAV audio: {err}")))?;
    let spec = reader.spec();

    if spec.channels == 0 {
        return Err(AppError::bad_request("WAV file has zero channels."));
    }

    let raw_samples = match spec.sample_format {
        hound::SampleFormat::Float => {
            let mut out = Vec::new();
            for sample in reader.samples::<f32>() {
                out.push(sample.map_err(|err| {
                    AppError::bad_request(format!("Failed to read WAV samples: {err}"))
                })?);
            }
            out
        }
        hound::SampleFormat::Int => {
            let scale = max_pcm_value(spec.bits_per_sample);
            let mut out = Vec::new();
            for sample in reader.samples::<i32>() {
                let value = sample.map_err(|err| {
                    AppError::bad_request(format!("Failed to read WAV samples: {err}"))
                })?;
                out.push((value as f32 / scale).clamp(-1.0, 1.0));
            }
            out
        }
    };

    let mono = mix_to_mono(raw_samples, spec.channels as usize);
    let duration_ms = ((mono.len() as f64 / spec.sample_rate as f64) * 1000.0).round() as u64;

    Ok(DecodedAudio {
        sample_rate: spec.sample_rate as i32,
        samples: mono,
        duration_ms,
    })
}

fn max_pcm_value(bits_per_sample: u16) -> f32 {
    let exponent = bits_per_sample.saturating_sub(1) as u32;
    ((1_i64 << exponent) - 1) as f32
}

fn mix_to_mono(samples: Vec<f32>, channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples;
    }

    let mut mono = Vec::with_capacity(samples.len() / channels.max(1));
    for frame in samples.chunks(channels) {
        let sum: f32 = frame.iter().copied().sum();
        mono.push(sum / frame.len() as f32);
    }
    mono
}

async fn healthz(State(state): State<AppState>) -> Result<Json<HealthResponse>, AppError> {
    Ok(Json(HealthResponse {
        ok: true,
        ready: true,
        model_family: "paraformer-zh-small".to_owned(),
        model_root: state.engine.model_root.display().to_string(),
        model_path: state.engine.model_path.display().to_string(),
        tokens_path: state.engine.tokens_path.display().to_string(),
        punctuation_enabled: state.engine.punctuator.is_some(),
        punctuation_model_path: state
            .engine
            .punctuation_model_path
            .as_ref()
            .map(|path| path.display().to_string()),
        default_language: state.engine.default_language.clone(),
        provider: state.engine.provider.clone(),
        threads: state.engine.threads,
    }))
}

async fn index_html() -> Html<&'static str> {
    Html(EMBEDDED_INDEX_HTML)
}

async fn transcribe(
    State(state): State<AppState>,
    Query(query): Query<TranscribeQuery>,
    body: Bytes,
) -> Result<Json<TranscriptionResponse>, AppError> {
    if body.is_empty() {
        return Err(AppError::bad_request("Request body is empty."));
    }

    let body = body.to_vec();
    let language = query.language.clone();
    let engine = state.engine.clone();

    let response =
        tokio::task::spawn_blocking(move || engine.transcribe(&body, language.as_deref()))
            .await
            .map_err(|err| AppError::internal(format!("Transcription task failed: {err}")))??;

    Ok(Json(response))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "voicebox_asr=info,tower_http=info".to_owned()),
        )
        .init();

    let args = Args::parse();
    let engine = AsrEngine::load(&args)?;
    info!("Resolved model root: {}", engine.model_root.display());
    info!("ASR model: {}", engine.model_path.display());
    info!("Tokens: {}", engine.tokens_path.display());
    if let Some(path) = &engine.punctuation_model_path {
        info!("Punctuation model: {}", path.display());
    } else {
        info!("Punctuation model: disabled");
    }

    let state = AppState {
        engine: Arc::new(engine),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(index_html))
        .route("/index.html", get(index_html))
        .route("/healthz", get(healthz))
        .route("/transcribe", post(transcribe))
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from((args.host, args.port));
    info!("VoiceBox ASR server listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        error!("Failed to listen for shutdown signal: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_model_restores_chinese_punctuation() {
        let model_path = PathBuf::from(DEFAULT_PUNCT_MODEL_DIR).join(DEFAULT_PUNCT_MODEL_FILE);
        if !model_path.exists() {
            return;
        }

        let punctuator =
            AsrEngine::create_punctuator(&model_path, 1, "cpu", false).expect("create punctuator");
        let text = punctuator
            .add_punctuation("我们都是木头人不会说话不会动")
            .expect("punctuate");

        assert_ne!(text, "我们都是木头人不会说话不会动");
        assert!(text.contains('，') || text.contains('。') || text.contains('？'));
    }
}
