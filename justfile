# aa_session 常用命令（https://github.com/casey/just）
# 用法：`just` 列出全部配方，`just <名字>` 执行。
#
# 两个容易踩的坑：
#
# 1. just 只做**文本替换**，不会创建同名 shell 变量。想要 shell 的默认值展开，
#    必须先 `out="{{out}}"` 赋值，再写 `${out:-...}`；直接写 ${out:-...} 会
#    永远取默认值（用户传了路径也不生效）。
# 2. 替换发生在 bash 看到脚本**之前**，所以插值进 shell 的参数一律要用
#    {{quote(...)}} 转义，否则 `just find 'a$b'` 会被 bash 当变量、
#    `just find '`id`'` 会被当命令执行。

# 二进制对应的 workspace 包名（与 Cargo.toml 一致）
cli_pkg := "aa_session_cli"

# ---------------------------------------------------------------- 常用查询

# 列出所有项目及其会话数（默认只统计顶层、未归档的会话）
projects *args:
    cargo run -q -p {{ cli_pkg }} -- --app opencode projects {{ args }}

# 列出会话。注意 --project 只收绝对路径（精确），模糊用 --project-like
sessions *args:
    cargo run -q -p {{ cli_pkg }} -- --app opencode sessions {{ args }}

# 列出所有会话运行目录（与项目不是一回事，见 AGENTS.md）
directories *args:
    cargo run -q -p {{ cli_pkg }} -- --app opencode directories {{ args }}

# 导出会话为 JSON：just dump <SESSION_ID> [输出路径]
# 不给路径时落到 exports/<ID>.json，随后可用 `just find` 搜。
dump session_id out="":
    #!/usr/bin/env bash
    set -euo pipefail
    # 必须先把 just 参数赋给 shell 变量再用 ${out:-}：just 只做文本替换，
    # 不会创建名为 out 的 shell 变量，直接写 ${out:-...} 会永远取默认值，
    # 用户传了输出路径也不生效。
    session_id="{{ session_id }}"
    out="{{ out }}"
    path="${out:-$PWD/exports/${session_id}.json}"
    mkdir -p "$(dirname "$path")"
    cargo run -q -p {{ cli_pkg }} -- --app opencode export {{ quote(session_id) }} -o "$path" --force

# 导出后直接用 Zed 打开（导出物是可直接全文搜索的裸文本 JSON）
zed session_id out="":
    #!/usr/bin/env bash
    set -euo pipefail
    session_id="{{ session_id }}"
    out="{{ out }}"
    path="${out:-$PWD/exports/${session_id}.json}"
    just dump {{ quote(session_id) }} "$path" > /dev/null
    zed "$path"

# 在导出目录里全文搜索（等价于 Zed 的 cmd+f，但可脚本化）
# 传单个关键词，或用 /正则/ 语法。可用 AA_SESSION_EXPORTS 指定别的目录。
find pattern:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="${AA_SESSION_EXPORTS:-$PWD/exports}"
    if [ ! -d "$dir" ]; then
        echo "导出目录不存在：$dir" >&2
        echo "先运行：just dump <SESSION_ID>" >&2
        exit 1
    fi
    rg --no-heading --color=never --line-number --glob '*.json' {{ quote(pattern) }} "$dir"

# ---------------------------------------------------------------- 开发

# 编译 release 版
build:
    cargo build --release

# 类型检查
check:
    cargo check --workspace

# 跑测试
test:
    cargo test --workspace

# clippy
lint:
    cargo clippy --workspace --all-targets

# 格式化 Rust 源码
fmt:
    cargo fmt --all

# 格式化 justfile 本身
fmt-just:
    just --fmt

# 提交前跑这个：justfile 格式 + Rust 格式 + clippy + 测试
ci:
    just --fmt --unstable --check
    cargo fmt --all --check
    cargo clippy --workspace --all-targets
    cargo test --workspace

# 透传参数给二进制：just run -- sessions --limit 5
run *args:
    cargo run -q -p {{ cli_pkg }} -- {{ args }}

# 干净重建
clean:
    cargo clean

# 列出全部配方
help:
    @just --list --unsorted
