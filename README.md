# Codex-O

Codex-O 是面向 Codex CLI 中文用户的本机优先桌面应用，使用 Tauri 2、
React、TypeScript 和 Rust 构建。项目聚焦两个能力域：

1. Skill Intelligence & Management：发现、解析、解释、比较和安全管理 Codex Skill。
2. Codex Sessions & Usage：预览、下载、安全删除 Codex 会话，并分析 Token 使用情况。

## 技术栈

- 桌面框架：Tauri 2
- 前端：React 19、TypeScript、Vite、React Router、Lucide React
- 本地后端：Rust
- 质量工具：ESLint、Vitest、Cargo test、GitHub Actions

默认桌面窗口为 `1280 × 800`，最小尺寸为 `1024 × 700`。

## 快速开始

环境要求：

- Node.js 22 或兼容版本
- npm 10 或兼容版本
- Rust/Cargo 稳定工具链
- macOS 上可用的 Xcode Command Line Tools

安装依赖并启动桌面开发模式：

```bash
npm ci
npm run tauri dev
```

开发模式会先启动 Vite，再运行 Tauri 原生窗口。请勿在该项目中提交密钥、真实
Codex 会话内容或本机绝对路径。

## 常用命令

| 命令 | 用途 |
|---|---|
| `npm run dev` | 仅启动 Vite 前端开发服务 |
| `npm run tauri dev` | 启动 Tauri 桌面开发模式 |
| `npm run lint` | 执行 ESLint |
| `npm run typecheck` | 执行 TypeScript 类型检查 |
| `npm test -- --run` | 执行 Vitest 测试 |
| `npm run check` | 组合执行 lint、类型检查、测试和进度检查 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 检查 Rust 格式 |
| `cargo test --manifest-path src-tauri/Cargo.toml` | 执行 Rust 单元测试 |

## 构建与发布

前端静态资源可通过以下命令生成：

```bash
npm run build
```

桌面应用的打包命令为：

```bash
npm run tauri build
```

发布前应先通过全部质量门禁，并依据[技术设计文档](doc/技术设计文档.md)和
[实施计划](doc/实施计划.md)确认目标平台、签名、版本号和发布流程。不得将密钥、
真实 Codex 数据或本机隐私信息打入产物。

## 页面信息架构

| 路由 | 页面 |
|---|---|
| `/skills` | 我的 Skills |
| `/skills/:skillId` | Skill 详情 |
| `/market` | Skill 市场 |
| `/install` | 安装 Skill |
| `/updates` | 更新中心 |
| `/sessions` | 会话管理 |
| `/token-stats` | Token 统计 |
| `/settings` | 设置 |
| `/mcp` | MCP 管理 |

## 目录说明

```text
src/
  app/           路由清单与前端测试
  components/    应用壳、侧栏、顶栏和页面状态组件
  App.tsx        路由入口
src-tauri/
  src/           Rust 应用入口与单元测试
  capabilities/  Tauri 权限声明
scripts/
  check-progress.mjs  PROGRESS.md 结构检查
doc/              产品、需求、技术设计和实施计划
prototype/        只读 HTML 原型交付包，仅作视觉与交互参考
```

## 质量门禁

本项目的基础验收命令如下：

```bash
npm ci
npm run lint
npm run typecheck
npm test -- --run
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
npm run check
node scripts/check-progress.mjs PROGRESS.md
```

前端测试验证九个正式路由的 ID 和路径完整且唯一；Rust 测试验证应用元数据常量；
进度检查脚本验证 `PROGRESS.md` 的必需章节和变更记录字段。

## 安全边界

- 前端不直接访问任意文件路径、Codex SQLite 或系统 Keyring。
- 文件、SQLite、路径校验和写操作必须在 Rust 安全边界内完成。
- 不在前端持久化绝对路径、API Key 或 confirmation token 历史。
- 写操作必须遵循 `plan -> confirm -> execute`；除受管 User Skill 外默认只读。
- 删除、恢复和永久清理仅能基于隔离 fixture 验证，除非获得对真实数据操作的明确授权。

## 相关文档与治理记录

- [产品蓝图与领域模型](doc/产品蓝图与领域模型.md)
- [需求文档](doc/需求文档.md)
- [技术设计文档](doc/技术设计文档.md)
- [实施计划](doc/实施计划.md)
- [开发进度](PROGRESS.md)
- [阻塞与待裁决](BLOCKED.md)

项目执行规范以 [AGENTS.md](AGENTS.md) 为准；原型目录的使用规则见
[prototype/AGENTS.md](prototype/AGENTS.md)。
