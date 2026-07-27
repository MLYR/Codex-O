# Codex-O 设计令牌（方案A Linear 定制 · 浅色优先）

> 本文档是 Phase 3 原型构建的唯一设计依据。token 命名统一为 `--color-* / --font-* / --space-* / --radius-* / --shadow-* / --motion-* / --z-* / --layout-*`，CSS 变量块可直接粘贴使用。

## 0. 设计基因速览

- **哲学**：冷灰中性承载信息密度，靛蓝紫点缀关键动作；层级靠灰度与字重，不靠色彩与阴影
- **三秒法则落点**：状态角标用「圆点+文字」双编码；版本号/路径/数字全部等宽；卡片单信息量 ≤5 元素
- **克制红线**：全页同时出现的彩色元素 ≤3 类；阴影仅用于浮层；危险色只在确认路径上出现

## 1. 完整 CSS 变量块（:root，可直接使用）

```css
:root {
  /* ===== 主色（Linear Indigo）===== */
  --color-primary: #5E6AD2;
  --color-primary-hover: #4F5BCB;
  --color-primary-active: #4653B8;
  --color-primary-text: #4653B8;
  --color-primary-subtle: #EEF0FC;
  --color-primary-border: #C7CDF0;

  /* ===== 中性色阶 ===== */
  --color-bg: #FFFFFF;
  --color-bg-sidebar: #F7F8F9;
  --color-surface: #FFFFFF;
  --color-surface-hover: #F5F6F8;
  --color-surface-sunken: #FBFBFD;
  --color-border: #E5E7EB;
  --color-border-strong: #D6D9E0;
  --color-divider: #EEEFF2;
  --color-text-primary: #282A30;
  --color-text-secondary: #62666D;
  --color-text-tertiary: #9CA3AF;
  --color-text-inverse: #FFFFFF;
  --color-overlay: rgba(20, 22, 35, 0.40);

  /* ===== 语义色三件套 ===== */
  --color-success: #4CB782;
  --color-success-text: #1F7A4D;
  --color-success-subtle: #E9F7F0;
  --color-success-border: #B7E4CE;

  --color-warning: #D97706;
  --color-warning-text: #B45309;
  --color-warning-subtle: #FDF3E3;
  --color-warning-border: #F3D9AE;

  --color-danger: #EB5757;
  --color-danger-text: #C93636;
  --color-danger-subtle: #FDECEC;
  --color-danger-border: #F5C6C6;
  --color-danger-action: #D13D3D;
  --color-danger-action-hover: #C93636;
  --color-danger-action-active: #B32E2E;

  --color-info: #4EA7FC;
  --color-info-text: #1D6FD8;
  --color-info-subtle: #EAF4FE;
  --color-info-border: #C2E0FB;

  /* ===== 图表色序 ===== */
  --chart-1: #5E6AD2; --chart-2: #4EA7FC; --chart-3: #4CB782; --chart-4: #E8B339;
  --chart-5: #EB5757; --chart-6: #9B8AF2; --chart-7: #38B2CE; --chart-8: #8A8F98;
  --chart-grid: #EEEFF2;
  --chart-area: rgba(94, 106, 210, 0.08);

  /* ===== 字体 ===== */
  --font-sans: Inter, -apple-system, BlinkMacSystemFont, "PingFang SC",
    "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif;
  --font-mono: "JetBrains Mono", "SF Mono", SFMono-Regular, "Cascadia Code",
    Consolas, "Liberation Mono", monospace;

  --font-size-display: 24px;
  --font-size-title: 20px;
  --font-size-h2: 15px;
  --font-size-h3: 13px;
  --font-size-body: 13px;
  --font-size-small: 12px;
  --font-size-micro: 11px;

  --font-weight-regular: 400;
  --font-weight-medium: 500;
  --font-weight-semibold: 600;

  --line-height-tight: 1.3;
  --line-height-title: 1.4;
  --line-height-body: 1.6;
  --line-height-small: 1.5;

  /* ===== 间距（4px 基准）===== */
  --space-1: 4px;  --space-2: 8px;  --space-3: 12px; --space-4: 16px;
  --space-5: 20px; --space-6: 24px; --space-8: 32px; --space-10: 40px; --space-12: 48px;

  /* ===== 圆角 ===== */
  --radius-xs: 4px;
  --radius-sm: 6px;
  --radius-md: 8px;
  --radius-lg: 12px;
  --radius-full: 999px;

  /* ===== 阴影 ===== */
  --shadow-xs: 0 1px 2px rgba(16, 18, 28, 0.05);
  --shadow-sm: 0 1px 2px rgba(16, 18, 28, 0.04), 0 1px 3px rgba(16, 18, 28, 0.06);
  --shadow-md: 0 4px 16px rgba(16, 18, 28, 0.08);
  --shadow-lg: 0 8px 32px rgba(16, 18, 28, 0.12);
  --shadow-focus: 0 0 0 3px rgba(94, 106, 210, 0.25);
  --shadow-focus-danger: 0 0 0 3px rgba(235, 87, 87, 0.22);

  /* ===== 层级 ===== */
  --z-sticky: 20; --z-banner: 30; --z-sidebar: 40;
  --z-dropdown: 50; --z-modal: 60; --z-toast: 70;

  /* ===== 布局 ===== */
  --layout-sidebar-width: 232px;
  --layout-topbar-height: 48px;
  --layout-content-padding: 24px;
  --layout-content-max: 960px;

  /* ===== 动效 ===== */
  --motion-ease-out: cubic-bezier(0.22, 1, 0.36, 1);
  --motion-ease-standard: cubic-bezier(0.2, 0, 0, 1);
  --motion-fast: 120ms;
  --motion-base: 200ms;
  --motion-slow: 300ms;
}

[data-theme="dark"] {
  --color-bg: #0F1015;
  --color-bg-sidebar: #13141B;
  --color-surface: #191B23;
  --color-surface-hover: #1F222B;
  --color-surface-sunken: #12131A;
  --color-border: #262933;
  --color-border-strong: #343845;
  --color-divider: #20232C;
  --color-text-primary: #E8E9ED;
  --color-text-secondary: #9BA0AB;
  --color-text-tertiary: #62677A;
  --color-text-inverse: #17181F;
  --color-overlay: rgba(0, 0, 0, 0.55);
  --color-primary: #8B93E8;
  --color-primary-hover: #9CA4EE;
  --color-primary-active: #7C84E0;
  --color-primary-text: #A6ADEF;
  --color-primary-subtle: rgba(139, 147, 232, 0.14);
  --color-primary-border: rgba(139, 147, 232, 0.35);
  --color-success-text: #6FCD9F;
  --color-success-subtle: rgba(76, 183, 130, 0.12);
  --color-success-border: rgba(76, 183, 130, 0.30);
  --color-warning-text: #ECB35C;
  --color-warning-subtle: rgba(224, 160, 64, 0.12);
  --color-warning-border: rgba(224, 160, 64, 0.30);
  --color-danger-text: #F07B7B;
  --color-danger-subtle: rgba(235, 87, 87, 0.12);
  --color-danger-border: rgba(235, 87, 87, 0.32);
  --color-info-text: #7BBDFD;
  --color-info-subtle: rgba(78, 167, 252, 0.12);
  --color-info-border: rgba(78, 167, 252, 0.30);
  --shadow-xs: 0 1px 2px rgba(0, 0, 0, 0.3);
  --shadow-sm: 0 1px 3px rgba(0, 0, 0, 0.4);
  --shadow-md: 0 4px 16px rgba(0, 0, 0, 0.45);
  --shadow-lg: 0 8px 32px rgba(0, 0, 0, 0.5);
}
```

## 2. 排版规范

| 场景 | 字体 | 说明 |
|------|------|------|
| 全部中文/西文 UI 文案 | `--font-sans` | Inter 管西文数字，PingFang/雅黑/Noto 管中文 |
| 路径、版本号、Token 数字、命令、代码、kbd | `--font-mono` + `font-variant-numeric: tabular-nums` | 统计大数字也必须 mono |
| 图标 | 线性 SVG 图标（1.5px 描边，Lucide/Feather 风格） | 禁用 emoji 作图标 |

| 层级 | 规格 | 用途 |
|------|------|------|
| Display | 24px/600/1.3/mono | 统计卡大数字、空态标题（空态用 sans） |
| Title | 20px/600/1.3 | 页面标题 |
| H2 | 15px/600/1.4 | 区块标题 |
| H3 | 13px/600/1.4 | 卡片标题、弹窗标题可用 15px |
| Body | 13px/400/1.6 | 正文、列表主文案 |
| Small | 12px/400/1.5 | 辅助说明、元数据 |
| Micro | 11px/500/1.4 | 徽章、角标、表头、时间戳 |

字重纪律：600 只给标题与关键数字；正文 400；徽章/表头 500；禁用 700+。

## 3. 组件规范

### 3.1 侧边栏
- 宽 232px，bg `--color-bg-sidebar`，右边框 1px `--color-border`
- 顶部产品区：高 48px，产品名 14px/600 + 版本号 mono 11px tertiary
- 导航分组标签：11px/500 tertiary，padding `12px 12px 4px`
- 导航项：高 32px，padding `0 12px`，radius-sm，图标 16px + 文字 13px/500
  - 默认：文字 secondary；hover：bg surface-hover
  - 选中：bg `--color-primary-subtle`，文字与图标 `--color-primary-text`
  - 项内右侧可挂 micro 角标（更新数、MCP 的「P2」灰标签）
- 底部：设置入口 + AI 服务状态简版（圆点 8px + 12px 文字）

### 3.2 顶栏
- 高 48px，bg `--color-bg`，下边框 `--color-border`
- 左：页面标题 15px/600；右：全局 AI 状态指示（圆点 8px + 12px/500 文字）
- AI 状态三态：正常=success「AI 正常」／降级=warning「AI 降级中」／不可用=danger「AI 不可用」

### 3.3 卡片
**通用卡片**：bg surface，边框 1px `--color-border`，radius-md，padding 16px，无阴影。hover（可点击卡）：边框变 border-strong + shadow-sm，过渡 120ms。

**Skill 卡片解剖**（网格内宽约 320px，高≈148px）：
```
┌────────────────────────────────┐
│ [图标32px]  名称 13px/600      [状态角标] │
│             版本 v1.2.0 mono 11px tertiary │
│ 中文一句话作用 12px secondary，2行截断      │
│ ─────────────────────────────│
│ [来源标签][P0/P1]    更新时间 11px tertiary │
└────────────────────────────────┘
```
- 图标占位：32×32，radius-md，bg primary-subtle，内放名称首字符 13px/600 primary-text
- 分区标题（用户/系统 Skills）：H2 + 数量 micro 角标，区间距 24px

### 3.4 按钮（高 32px 默认）
| Variant | 默认 | Hover | Disabled |
|---------|------|-------|----------|
| Primary | bg primary 白字 | primary-hover | bg #EFF0F3，字 tertiary |
| Secondary | bg 白，边框 border-strong，字 primary | bg surface-hover | 同上 |
| Danger | bg danger-action 白字 | danger-action-hover | 同 Primary |
| Ghost | 透明，字 secondary | bg surface-hover | 字 tertiary |

- radius-sm，padding `0 12px`，13px/500，图标 14px 间距 6px
- 小尺寸 sm：高 28px，padding `0 10px`
- focus：`--shadow-focus`（危险用 `--shadow-focus-danger`）
- 加载中：左侧 12px spinner + 文字不变，禁用点击

### 3.5 标签/徽章
**来源标签**（高 20px，radius-xs，11px/500，padding `0 6px`）：
| 来源 | 文字 | 浅底 | 边框 |
|------|------|------|------|
| 用户自建 | primary-text | primary-subtle | primary-border |
| 系统内置 | text-secondary | #F3F4F6 | border |
| 市场安装 | info-text | info-subtle | info-border |

**状态角标**：圆点 6px + 11px/500 文字，浅底 pill padding `2px 8px` radius-full。

**数字角标**：min-width 16px 高 16px radius-full，bg border-strong 白字 10px/600 mono；可更新数用 info 色。

### 3.6 搜索框与输入
- 搜索框：高 32px，宽 240–320px，bg surface-sunken，边框 border，radius-sm；左图标 14px；placeholder tertiary；右侧 kbd `⌘K`
- focus：bg 转白 + 边框 primary + shadow-focus
- 表单输入：同搜索框，label 12px/500 secondary 在上；校验失败边框 danger-text + shadow-focus-danger
- 下拉菜单：bg 白 radius-md shadow-md，项高 32px hover surface-hover，选中前缀 ✓ primary

### 3.7 Tabs 与分段控件
- Tabs：容器下边框 divider；项 padding `8px 12px` 13px/500 secondary；选中字 primary 600 + 底部 2px primary 指示条
- 分段控件：容器 bg #EFF0F3 radius-sm padding 2px；项高 24px padding `0 10px` radius-xs 12px/500；选中 bg 白 + shadow-xs

### 3.8 列表/表格/折叠组
- 表头：11px/500 secondary，下边框 border，padding `8px 12px`
- 行：高 40px，padding `0 12px`，下边框 divider；hover bg surface-hover；mono 列 12px
- 项目分组折叠组：组头高 36px，chevron 14px（展开旋转 90° 200ms）+ 项目名 13px/600 + 会话数 micro 角标 + 路径 mono 11px tertiary
- 会话卡片：白卡 radius-md padding `12px 16px`；标题 13px/500 单行截断；meta 行 mono 11px tertiary

### 3.9 AI 三要素区块（详情页核心）
- 容器：bg primary-subtle，边框 1px primary-border，radius-md，padding 16px
- 头行：AI 图标 16px primary + 「AI 中文解析」标签 + 右侧「重新解析」Ghost sm
- 「作用」Body 13px/1.6；「使用场景」列表，每项前 4px 圆点 primary，行间距 6px；「使用方式」mono 代码块（bg surface-sunken，radius-sm，padding `8px 12px`，12px/1.5）
- **降级态**：整体切换 warning 三件套，标签改「AI 降级 · 展示原文」，展示英文原文 + 「重试解析」Secondary sm

### 3.10 骨架屏
- 基色 #EFF0F3，shimmer 高光 linear-gradient(90deg, transparent, rgba(255,255,255,0.7), transparent)，1.2s 线性循环
- 文本条：高 12px radius-xs，宽度 60%/80%/40% 错落；卡片骨架还原真实卡片解剖
- 列表页首屏：3×3 卡片骨架网格

### 3.11 弹窗
- 遮罩 `--color-overlay`，z-modal；面板 bg 白 radius-lg shadow-lg padding 20px
- 常规宽 480px；标题 15px/600，正文 13px secondary；按钮区右对齐间距 8px
- 进入：fade + scale 0.96→1 + translateY(4px→0)，200ms ease-out
- **卸载二次确认**（危险范式）：
  - 标题前 danger 警示图标 16px；标题「卸载 skill-name？」（名称 mono）
  - 目录路径块：bg surface-sunken，radius-sm，padding `8px 12px`，mono 12px
  - 附属文件清单：「将同时删除以下 N 个文件」+ 列表 mono 11px，max-height 160px 内部滚动
  - 警示条：danger 三件套 callout「此操作不可撤销」
  - 按钮：左「取消」Secondary，右「确认卸载」Danger（默认焦点在取消）

### 3.12 Toast
- 右下距边 24px，z-toast；宽 320px，bg #1F2128，字 #F5F6F8 13px，radius-md，shadow-lg，padding `12px 16px`
- 左侧类型图标 16px；进入 translateY(8px)+fade 200ms，退出 150ms；自动消失 4s

### 3.13 横幅（全局 AI 降级）
- 顶栏下方通栏，高 36px，warning 三件套
- 左：警示图标 + 「AI 服务降级中，Skill 解析展示英文原文」；右：「重试连接」文字按钮 + 关闭 ×
- 展开/收起：高度过渡 250ms

### 3.14 统计卡（×5）
- 白卡 padding 16px；label 12px secondary；数值 24px/600 mono tabular-nums；单位 12px tertiary
- 副行环比 chip 11px mono（涨 success / 降 danger）
- grid `repeat(5, 1fr)` gap 12px，单卡 min-width 160px

### 3.15 图表
- 分布图：横向条形或环图，分类色按 --chart-1…8 顺序取，>8 类合并「其他」
- 趋势折线：主线 --chart-1 2px + 渐变面积 --chart-area；网格 --chart-grid 水平虚线；轴标签 mono 11px tertiary
- Top N 排行：行高 28px，名称 12px 截断 + 条形 bg primary-subtle + 数值 mono 12px
- Tooltip：bg #1F2128 白字 12px radius-sm padding `6px 10px`

### 3.16 空态
- 居中，图标 48px 置于 80×80 圆形容器（bg #EFF0F3，图标 tertiary）
- 标题 14px/600；描述 12px secondary max-width 320px；主 CTA Primary + 次级 Ghost
- 错误态变体：图标容器 danger-subtle，CTA 改「重试」

## 4. 七态状态色映射

| 状态 | 圆点 | 文字 | 浅底 | 深边框 | 应用 |
|------|------|------|------|--------|------|
| 正常/已启用 | success | success-text | success-subtle | success-border | 运行中角标 |
| 已禁用 | text-tertiary | text-secondary | #F3F4F6 | border | 灰底角标；整卡不降透明度 |
| 可更新 | info | info-text | info-subtle | info-border | 「可更新 v1.2.0→v1.3.0」mono |
| AI 降级 | warning | warning-text | warning-subtle | warning-border | 全局横幅 + 三要素区块 |
| 加载中 | primary spinner | — | #EFF0F3 骨架 | — | 骨架屏/按钮 spinner |
| 错误 | danger | danger-text | danger-subtle | danger-border | 错误条 + 重试 |
| 空 | text-tertiary | text-secondary | #EFF0F3 | 虚线 border-strong | 空态引导 |

## 5. 布局栅格

- 窗口基准 1280×800（最小 1024×640）；侧边栏 232px + 顶栏 48px
- 内容区 padding 24px；列表/统计/市场页通栏流式；详情/设置/安装页 max-width 960px 左对齐
- Skill 卡片网格：`repeat(auto-fill, minmax(300px, 1fr))` gap 16px（1280 下 3 列）
- 节奏：标题区 → 24px → 工具栏 → 16px → 内容 → 区块间 32px

## 6. 动效基调

| 场景 | 时长 | 缓动 |
|------|------|------|
| hover/颜色过渡 | 120ms | ease-out |
| 折叠展开/Tab | 200ms | standard |
| 弹窗/Toast/下拉 | 200ms（退150ms） | ease-out |
| 横幅滑入 | 250ms | standard |
| 图表绘制 | ≤300ms | ease-out |
| 骨架 shimmer | 1200ms 循环 | linear |
| AI 连接中脉冲 | 2000ms 循环 | ease-in-out |

`prefers-reduced-motion` 降级：全部动画/过渡时长压至 0.01ms，shimmer 停为静态，脉冲点静态。

## 7. 禁区与偏好

**禁止**：大面积渐变、彩色背景放正文、纯 #000 文本、平面堆叠阴影、emoji 图标、13px 以下正文、危险色大面积填充、同屏彩色语义 >3 类、已禁用整卡降透明度。

**偏好**：层级优先级 灰度>字重>边框>阴影>色彩；技术值走 mono+tabular-nums；危险操作三段式（警示图标+路径清单+默认焦点取消）；状态双编码（色点+文字）。
