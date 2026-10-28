# Local AI models

Open **Settings → AI** to choose Apple Intelligence, a GGUF model run by
llama.cpp (**llama.cpp** tab), or, on Apple silicon Macs, an MLX model run by
mlx-lm (**MLX** tab). Both local tabs can use an existing model or download one
from Hugging Face. The selection is saved across restarts
and applies to intelligent search, document summaries, and AI-generated text.
Image recognition/OCR still uses macOS Vision; selecting a text model does not
add multimodal support or enable Vision on other platforms.

## Thinking limits

In **Settings → AI → Thinking**, choose **None**, **Low (default)**, **Medium**,
**High**, or **Unlimited**. The preference is saved and applies to subsequent
local-model requests without reloading the model.

For GGUF, these select thinking budgets of 0, 64, 128, 256, or unrestricted
tokens respectively, within the existing 2,048-token total response allowance.
None also requests non-thinking mode. A recent llama.cpp runtime closes
the thinking block when the budget is reached and continues with the answer,
rather than stopping the response. Older custom runtimes may ignore the budget;
update the runtime if thinking remains lengthy. This is not a seconds-based cap:
generation speed depends on the model and hardware.

For MLX, None sets `chat_template_kwargs.enable_thinking` to `false`, requesting
direct answers from templates that support it; all other levels enable thinking.
mlx-lm does not provide the same separate reasoning-token budget, so Low, Medium,
High, and Unlimited behave alike. Models that ignore the switch can still think.
Apple Intelligence is unchanged.

## Runtime

Local inference requires [llama.cpp's llama-server](https://github.com/ggml-org/llama.cpp/tree/master/tools/server).
Choose **Local model**, then **Install runtime** in Settings → AI to download an official GitHub build
for your OS/architecture. Rile verifies its SHA-256 against GitHub release
metadata, extracts the complete runtime (including its libraries), tests that
`llama-server` starts, and saves the executable path automatically. No terminal,
Homebrew, or administrator access is needed. Progress and cancellation share
the model download status area. Failures leave your previous runtime selected.

Managed runtimes live in `runtimes/` beside `settings.toml` (respecting
`RILE_CONFIG_DIR`). Installs are versioned and never overwrite an existing
version. Clicking **Install runtime** again selects the newest compatible build,
reusing it if already installed. Older versions can be removed manually once
no longer in use. Official builds may be marked prerelease; Rile skips releases
that contain no compatible binary. Automatic installs support macOS, Windows,
and Ubuntu-compatible Linux on ARM64/x86-64; other platforms need a custom build.
Linux builds may require newer system libraries than your distribution provides.

Alternatively, install llama.cpp separately, enter the full executable path in
Settings under **Use a custom runtime…**, and click **Save path**. A Homebrew installation on Apple
Silicon normally uses `/opt/homebrew/bin/llama-server`. A bare `llama-server`
also works if it is on the application's PATH (Finder-launched apps may have a
different PATH from your terminal).

Rile launches the runtime itself, listening only on loopback on a temporary
port. No external inference service is used. While AI features are on, Rile
loads the selected model in the background when it starts and keeps it in
memory, so requests don't wait for it; the AI overview in Settings shows
**Loading … into memory** and then **loaded in memory**. A request made while
the model is still loading waits for that load. Switching to another model asks
for confirmation first, then unloads the previous model and loads the new one.
Choosing Apple Intelligence, turning AI features off, or quitting Rile frees
the memory. Errors such as missing executables, incompatible models, insufficient
memory, and load timeouts appear in the AI feature's existing error UI.

## Downloads

Under **Model**, each local tab lists recommended models by size: Qwen 3.5 2B,
Qwen 3.5 4B (recommended), Gemma 4 E4B, Qwen 3.5 9B and Qwen 3.8 27B — Q4_K_M
GGUF files from `unsloth/…-GGUF` for llama.cpp, 4-bit `mlx-community` builds for
MLX. Each shows its download size and the memory it needs; models needing more
memory than this computer has are flagged. **Download** fetches the model and
selects it once complete; progress, speed, time left and **Cancel** appear on
that model's row. Downloaded models move to **On this computer**, where clicking
one selects it and the trash button deletes it.

For any other model, choose **Custom from Hugging Face**, enter a public,
ungated `owner/repository` and click **Find Files** (llama.cpp) to list its
single-file GGUF models with sizes — Q4_K_M files are marked **Balanced** — or
**Download** (MLX) to fetch the whole repository. Rile downloads from the
repository's `main` revision. Use a chat/instruct GGUF supported by your
installed llama.cpp version, with its chat template embedded. Split models,
adapters, Safetensors, private/gated repositories, and separate vision
projectors are not offered by this flow. **Choose File…** uses a model already
on disk.

Check the model's license, download size, and RAM requirements on its model card
first. A quantized model may still require several GB of storage and RAM.
Model and runtime downloads show a progress bar, transferred/total size, and
percentage when a total is available. Unknown totals show an animated bar and
transferred size instead. **Cancel** stays beside the progress indicator.
Incomplete files are removed
on cancellation/error; only completed GGUF files are selected. Existing cached
files are reused. Downloads live in `models/` beside `settings.toml` (respecting
`RILE_CONFIG_DIR`). To reclaim disk space, switch to another model, then remove
unused files from that directory. After a crash, a leftover `.gguf.part` file can
be removed before retrying.

Choosing **Apple Intelligence** restores the original system backend, which
still requires a supported Mac and enabled Apple Intelligence. Local GGUF
inference does not require Apple Intelligence or the `ai` Cargo feature.

## MLX models (Apple silicon)

The **MLX** tab runs models in Apple's [MLX](https://github.com/ml-explore/mlx)
format with [mlx-lm](https://github.com/ml-explore/mlx-lm)'s `mlx_lm.server`.
It only appears on Macs with Apple silicon.

**Install Runtime** downloads the latest official
[uv](https://github.com/astral-sh/uv) build from GitHub (SHA-256 verified
against the release metadata), uses it to install a private Python 3.12 and
creates a virtual environment with the `mlx-lm` package from PyPI. Everything
lives in `runtimes/` beside `settings.toml`: `uv-<version>/`, `python/` and
`mlx-lm/` (the environment; `mlx-lm/bin/mlx_lm.server` is the executable).
No system Python, Homebrew or Terminal is used, and your own Python setups are
left alone. The first install downloads a few hundred MB; **Check for Updates**
upgrades `mlx-lm` in place. An interrupted first install is removed. To use an
existing installation instead (for example `pip install mlx-lm`), enter the path
to its `mlx_lm.server` under **Custom Path…**.

**Download…** takes only a Hugging Face `owner/repository`, such as
`mlx-community/Qwen3-4B-4bit` — the [mlx-community](https://huggingface.co/mlx-community)
organisation has many ready-converted chat models. Rile downloads the
repository's `main` revision: `config.json`, tokenizer files, chat templates
and `.safetensors` weights (top-level files only; READMEs, images and custom
Python code are skipped). Weights are checked against their SHA-256 from
Hugging Face. The model appears in `models/<owner>/<repository>/` only once
every file is complete; cancelled or failed downloads are removed. Repositories
without `config.json` and `.safetensors` weights (for example GGUF-only ones) are
rejected. **Choose Folder…** selects an MLX model folder you already have, such
as one converted with `mlx_lm.convert`.

As with llama.cpp, each request starts `mlx_lm.server` on a temporary loopback
port and stops it when finished or cancelled. MLX loads the model on the first
request, so the first response takes a moment. Memory needs are roughly the
size of the model folder plus working memory.
