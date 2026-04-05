# Codex Bridge

`codex-bridge` 是一个 Linux 优先的 Rust 主控程序。
它通过 NapCat 把桌面 QQ 接成消息收发信道，再把 Codex App Server 接到这条信道后面，形成一个带审批、队列、回复 skill 和本地 API 的 Agent Runtime。

当前唯一正式接入的 transport 是 QQ，但项目本身不把 QQ 当作唯一目标。

## 它做什么

- 前台启动 Linux QQ，并注入当前仓库构建出的 NapCat shell
- 通过正式 OneBot WebSocket 收发 QQ 私聊和群聊消息
- 启动本地 `codex app-server` 子进程，并通过 `stdio` 驱动
- 维护会话到 Codex thread 的长期绑定
- 提供单任务执行队列、管理员审批门、取消、重试和状态查询
- 暴露一组本地 HTTP API，供 CLI、skill 或其他本地程序调用

## 它不做什么

- 不重写 QQ 私有协议
- 不承诺零风控、零封号风险
- 不把 OneBot 术语当成主要用户接口
- 不提供系统级沙箱；当前是运行时策略约束，不是容器隔离

## 仓库结构

- `crates/codex-bridge-core`
  运行时、调度器、审批门、状态库、本地 API
- `crates/codex-bridge-cli`
  CLI 入口
- `skills/`
  项目级 skill，运行时会把 `.agents/skills` 软链接到这里
- `deps/NapCatQQ`
  固定版本的 NapCat 源码
- `deps/codex`
  固定版本的 Codex 源码
- `.run/`
  运行时目录、日志、状态库、prompt、admin 配置、artifact

## 依赖

运行和开发都基于 Linux。

需要：

- `node`
- `pnpm`
- `python3`
- `curl`
- `xvfb-run`
- `dpkg`
  或 `rpm2cpio + cpio`，仅在本机未安装 QQ 且需要自动安装时使用
- Rust 工具链
  当前仓库仍以源码运行和源码验证为主

默认 QQ 路径：

```text
$HOME/Napcat/opt/QQ/qq
```

如果这个二进制不存在，启动器会自动安装 Linux QQ。

## 快速开始

先拉 submodule：

```bash
git submodule update --init --recursive
```

然后直接启动：

```bash
cargo run -p codex-bridge-cli -- run
```

启动阶段会做这些事：

1. 构建 `deps/NapCatQQ` 里的 NapCat shell 产物
2. 准备 `.run/default/` 运行目录
3. 生成或复用 WebUI / WS token
4. 生成或复用 `system_prompt.md`
5. 生成或复用 `admin.toml`
6. 同步隔离态 `CODEX_HOME` 所需的 `config.toml` 和 `auth.json`
7. patch QQ 的加载入口，使其加载当前仓库构建产物
8. 前台启动 QQ + NapCat
9. 启动本地 API 和 Codex runtime

前台终端里会看到：

- QQ / NapCat 日志
- Rust 侧 bridge / orchestrator / Codex runtime 日志
- 文本二维码

扫码登录后，bot 才会真正开始接收和处理消息。

## 运行时目录

默认运行时根目录：

```text
.run/default/
```

关键文件：

- `.run/default/state.sqlite3`
  会话绑定、任务状态、任务历史
- `.run/default/prompt/system_prompt.md`
  当前全局唯一生效的人设 / system prompt 文件
- `.run/default/config/admin.toml`
  管理员 QQ 配置
- `.run/default/run/launcher.env`
  生成的 WebUI / WS token
- `.run/default/logs/launcher.log`
  QQ/NapCat 前台日志镜像
- `.run/default/codex-home/config.toml`
  隔离态 Codex 配置副本

artifact 目录：

```text
.run/artifacts/
```

新文件只能写到这里；skill 发图片和文件时也只允许从这里取。

## 配置

### 1. 管理员配置

运行时会自动创建：

```toml
# .run/default/config/admin.toml
admin_user_id = 2394626220
```

默认管理员就是 `2394626220`。

### 2. System Prompt

当前生效 prompt 文件：

```text
.run/default/prompt/system_prompt.md
```

这是唯一真相来源。
你可以直接改这个文件；后续新 turn 和 resume 都会使用它。

### 3. Codex 配置

为了隔离全局 skill 和运行状态，`codex-bridge` 会给 `codex app-server` 准备一个独立的：

```text
.run/default/codex-home/
```

启动时会尝试从你本机的 `CODEX_HOME` 复制：

- `config.toml`
- `auth.json`

如果你依赖自定义 `model_provider`、`base_url` 或其他 Codex 配置，这一步很重要。

## 本地 API

默认监听：

```text
http://127.0.0.1:36111
```

主要路由：

- `GET /health`
- `GET /api/session`
- `GET /api/friends`
- `GET /api/groups`
- `GET /api/status`
- `GET /api/queue`
- `POST /api/tasks/cancel`
- `POST /api/tasks/retry-last`
- `POST /api/reply`
- `POST /api/messages/private`
- `POST /api/messages/group`
- `GET /api/events/ws`

示例：

```bash
curl http://127.0.0.1:36111/health
curl http://127.0.0.1:36111/api/session
curl http://127.0.0.1:36111/api/friends
```

发送私聊：

```bash
curl -X POST http://127.0.0.1:36111/api/messages/private \
  -H 'content-type: application/json' \
  -d '{"user_id":2394626220,"text":"hello from codex-bridge"}'
```

订阅事件：

```text
ws://127.0.0.1:36111/api/events/ws
```

## CLI

启动：

```bash
cargo run -p codex-bridge-cli -- run
```

查看状态：

```bash
cargo run -p codex-bridge-cli -- status
cargo run -p codex-bridge-cli -- queue
```

发送消息：

```bash
cargo run -p codex-bridge-cli -- send-private --user-id 2394626220 --text "hello"
cargo run -p codex-bridge-cli -- send-group --group-id 123456 --text "hello group"
```

取消和重试：

```bash
cargo run -p codex-bridge-cli -- cancel
cargo run -p codex-bridge-cli -- retry-last
```

skill 回复当前会话：

```bash
cargo run -p codex-bridge-cli -- reply --text "处理完成了"
cargo run -p codex-bridge-cli -- reply --image .run/artifacts/result.png
cargo run -p codex-bridge-cli -- reply --file .run/artifacts/report.md
python3 skills/reply-current/reply_current.py --text "处理完成了"
```

## 消息触发规则

- 非好友私聊：直接拒绝，不进 Codex
- 好友私聊：
  - 如果发起人是 admin，直接执行
  - 否则先进入待审批池
- 群聊：
  - 只有 `@bot` 才触发
  - 如果发起人是 admin，直接执行
  - 否则先进入待审批池

控制命令：

- `/help`
- `/status`
- `/queue`
- `/cancel`
- `/retry_last`
- `/approve <task_id>`
- `/deny <task_id>`
- `/status <task_id>`

其中：

- `/approve`、`/deny`、`/status <task_id>` 只允许 admin 私聊使用
- `/cancel` 只能取消自己发起的当前任务
- `/retry_last` 只能重试自己在当前会话里的最近失败/中断任务

## 管理员审批流

这条规则是当前安全边界的核心：

- 只有 admin 私聊和 admin 在群里 `@bot` 能直接执行
- 其他所有可执行请求都要先审批

审批流程：

1. 请求进入 `pending approval` 池
2. bot 立即回原提问者一条“等待管理员确认”的提示
3. bot 私聊 admin 一条审批摘要
4. admin 私聊：
   - `/approve <task_id>`
   - `/deny <task_id>`
   - `/status <task_id>`
5. 若 `15` 分钟内没人处理，则自动 `Expired`

待审批任务不会占用现有 Codex 执行队列，只有批准后才会真正入队。

## 当前行为边界

- 全局同一时刻只跑 1 个 Codex 任务
- 等待队列上限 5
- 待审批池默认上限 32
- 正常成功结果应由 `reply-current` skill 主动回传
- 如果任务成功结束但没有任何 skill 回传，bridge 会发一条短兜底提示
- 群聊开始处理时会对原消息打一个敬礼表情
- 私聊开始处理时会发一条人设化短文本

## 安全边界

当前不是容器隔离，而是运行时策略约束：

- 全机可读
- 网络可用
- 当前仓库内已有文件可以修改
- 新文件只能写到 `.run/artifacts/`
- `kill`、`pkill`、`killall`、`shutdown`、`reboot`、`poweroff`、`systemctl stop/restart/kill` 会被拒绝
- `thread/shellCommand` 不使用
- prompt 里也明确禁止任何删除操作

## 技能系统

- 项目技能放在 `skills/`
- 运行时会把 `.agents/skills` 链到 `skills/`
- 当前统一结果回传 skill 是：

```text
skills/reply-current/SKILL.md
```

## 开发与验证

常用命令：

```bash
make fmt
make lint
make test
make run
```

等价验证：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace -- --nocapture
```

## CI

仓库内置 GitHub Actions CI，会在 `push` 和 `pull_request` 上执行：

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace -- --nocapture`

CI 会递归拉取 submodule。

## 许可证

本项目使用 [MIT License](LICENSE)。
