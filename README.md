# VoiceBox ASR

<p align="center">
  <img src="assets/icon.png" alt="VoiceBox ASR icon" width="160" height="160" />
</p>

本仓库提供一个独立安装的本地离线语音转写服务：Rust sidecar 自带前端测试页，并在 release 中打包 Paraformer 中文模型。

当前仓库核心内容：

- `src/main.rs`: 本地离线语音转写 sidecar，基于 `Rust + sherpa-onnx`
- `index.html`: 内嵌到服务里的录音和接口测试页
- `models/`: release 一起打包的离线模型资源
- `.github/workflows/release.yml`: GitHub Actions 跨平台发布工作流

## 运行服务

当前实现已经切到阿里达摩院 `Paraformer` 中文小模型路线，推荐准备这个模型目录：

- `csukuangfj/sherpa-onnx-paraformer-zh-small-2024-03-09`

目录里至少包含：

- `model.int8.onnx`
- `tokens.txt`

推荐目录布局：

```text
voicebox-asr
models/
  paraformer-zh-small-2024-03-09/
    model.int8.onnx
    tokens.txt
```

固定发布目录也按这个结构生成：

```text
dist/
  voicebox-asr/
    voicebox-asr
    README.txt
    models/
      paraformer-zh-small-2024-03-09/
        model.int8.onnx
        tokens.txt
```

在当前工程里，这两个文件已经放到了：

- `models/paraformer-zh-small-2024-03-09/model.int8.onnx`
- `models/paraformer-zh-small-2024-03-09/tokens.txt`

现在默认启动方式可以直接简化成：

```bash
cargo run
```

或者发布后的独立安装包里直接运行：

```bash
./voicebox-asr
```

如果要生成这个发布目录，直接运行：

```bash
./scripts/package-dist.sh
```

脚本会：

1. `cargo build --release`
2. 清理并重建 `dist/voicebox-asr/`
3. 复制 release 二进制和模型文件
4. 写入一个最小 `README.txt`

## GitHub Release

仓库默认发布形态是跨平台 archive 安装包，由 GitHub Actions 生成：

- Linux: `voicebox-asr-linux-x64.tar.gz`
- Windows: `voicebox-asr-windows-x64.zip`
- macOS Intel: `voicebox-asr-macos-x64.tar.gz`
- macOS Apple Silicon: `voicebox-asr-macos-arm64.tar.gz`

工作流文件：

- `.github/workflows/release.yml`

触发方式：

1. 手动触发 `workflow_dispatch`
2. 推送 tag，例如 `v0.1.0`

tag 发布时，workflow 会把对应 archive 直接上传到 GitHub Release。

程序会按下面顺序自动找模型：

1. `--model-dir` 指向的目录
2. `--model` / `--tokens` 所在目录
3. 可执行文件旁边的 `models/paraformer-zh-small-2024-03-09/`
4. 当前工作目录下的 `models/paraformer-zh-small-2024-03-09/`

如果你要显式覆盖路径，也可以这样启动：

```bash
cargo run -- \
  --model-dir /absolute/path/to/paraformer-zh-small-2024-03-09
```

或者保留原来的细粒度参数：

```bash
cargo run -- \
  --model /absolute/path/to/model.int8.onnx \
  --tokens /absolute/path/to/tokens.txt \
  --language zh \
  --threads 2
```

说明：

- 这是中文模型，服务当前只接受 `zh`，`auto/zh-cn/cmn` 这类别名会被归一化到 `zh`
- `--language` 现在只是声明默认语言，不再像 SenseVoice 那样切换多语言模型行为
- 独立安装时，把 `models/` 目录和 Rust 可执行文件放在同一级即可，不需要再传模型参数
- 建议把最终分发单元固定为 `dist/voicebox-asr/`，不要让用户自己拼目录

默认监听：

- `http://127.0.0.1:8765/`
- `http://127.0.0.1:8765/healthz`
- `http://127.0.0.1:8765/transcribe`

也可以用环境变量：

```bash
export VOICEBOX_MODEL=/absolute/path/to/model.int8.onnx
export VOICEBOX_TOKENS=/absolute/path/to/tokens.txt
export VOICEBOX_MODEL_DIR=/absolute/path/to/paraformer-zh-small-2024-03-09
export VOICEBOX_LANGUAGE=zh
cargo run
```

## 接口

### `GET /healthz`

返回服务状态、模型类型、模型根目录、模型路径、tokens 路径、默认语言和 provider。

### `POST /transcribe`

- 请求体：`audio/wav`
- 支持 `?language=zh` 覆盖默认语言
- 79M Paraformer 版本当前只支持中文
- 返回 JSON：

```json
{
  "ok": true,
  "text": "你好，这是转写结果",
  "language": "zh",
  "elapsed_ms": 320,
  "audio_duration_ms": 1800,
  "segments": [
    {
      "start_ms": 0,
      "end_ms": 820,
      "text": "你好"
    }
  ]
}
```

## 测试页面

现在测试页已经由 Rust 服务直接托管：

1. 启动 `cargo run` 或独立安装后的 `./voicebox-asr`
2. 打开 `http://127.0.0.1:8765/`
3. 页面会默认使用当前同源服务地址
4. 点击录音
5. 停止后页面会把 16k 单声道 WAV 发到 `POST /transcribe`
6. 转写成功后把文字插入输入框
