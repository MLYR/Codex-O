export function formatBytes(sizeBytes: number): string {
  if (sizeBytes < 1024) {
    return `${sizeBytes} B`;
  }
  if (sizeBytes < 1024 * 1024) {
    return `${(sizeBytes / 1024).toFixed(1)} KiB`;
  }
  return `${(sizeBytes / (1024 * 1024)).toFixed(1)} MiB`;
}

export function formatUpdatedAt(updatedAtMs?: number): string {
  if (!updatedAtMs) {
    return "时间未知";
  }
  return new Intl.DateTimeFormat("zh-CN", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(updatedAtMs));
}

export function scopeLabel(scope: string): string {
  const labels: Record<string, string> = {
    user: "用户",
    repository: "仓库",
    legacy_user: "旧用户目录",
    system: "系统",
    plugin: "插件",
    bundled: "内置",
    additional: "附加目录",
  };
  return labels[scope] ?? scope;
}

export function diagnosticLabel(code: string): string {
  const labels: Record<string, string> = {
    entry_unreadable: "文件不可读取",
    input_too_large: "文件超出读取上限",
    invalid_frontmatter: "Frontmatter 格式无效",
    invalid_path: "路径无效",
    invalid_utf8: "不是有效 UTF-8 文本",
    invalid_yaml: "YAML 格式无效",
    provider_unavailable: "来源不可用",
    root_unavailable: "扫描根目录不可用",
    symlink_denied: "符号链接已拒绝",
  };
  return labels[code] ?? code;
}
