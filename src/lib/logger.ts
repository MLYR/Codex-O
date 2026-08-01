import { error, info, warn } from "@tauri-apps/plugin-log";

import type { DiagnosticLevel, DiagnosticRecord } from "../features/settings/api";

type FrontendLogInput = Pick<DiagnosticRecord, "category" | "domain" | "event_code" | "result" | "module" | "retryable"> &
  Partial<Pick<DiagnosticRecord, "submodule" | "duration_ms" | "trace_id" | "request_ref" | "provider" | "model" | "http_status" | "item_count" | "error_code" | "recovery_code">>;

const safeIdentifier = /^[a-z0-9_.:-]{1,128}$/;
const safeHex = /^[0-9a-f]{1,64}$/;

function isSafeIdentifier(value: unknown, maxLength: number): value is string {
  return typeof value === "string" && value.length <= maxLength && safeIdentifier.test(value);
}

export async function logFrontendEvent(level: DiagnosticLevel, input: FrontendLogInput): Promise<void> {
  if (
    !isSafeIdentifier(input.module, 64) ||
    !isSafeIdentifier(input.event_code, 128) ||
    (input.submodule !== undefined && !isSafeIdentifier(input.submodule, 64)) ||
    (input.provider !== undefined && !isSafeIdentifier(input.provider, 64)) ||
    (input.model !== undefined && !isSafeIdentifier(input.model, 128)) ||
    (input.error_code !== undefined && !isSafeIdentifier(input.error_code, 64)) ||
    (input.recovery_code !== undefined && !isSafeIdentifier(input.recovery_code, 64)) ||
    (input.trace_id !== undefined && !safeHex.test(input.trace_id)) ||
    (input.request_ref !== undefined && !safeHex.test(input.request_ref))
  ) {
    return;
  }

  // Explicitly rebuild the payload so runtime callers cannot smuggle prompt, response, or other fields through the wrapper.
  const event = {
    schema_version: 1,
    event_id: `evt-frontend-${Date.now().toString(16)}`,
    occurred_at: Date.now(),
    level,
    redaction_version: 1,
    category: input.category,
    domain: input.domain,
    event_code: input.event_code,
    result: input.result,
    module: input.module,
    retryable: input.retryable,
    ...(input.submodule === undefined ? {} : { submodule: input.submodule }),
    ...(input.duration_ms === undefined ? {} : { duration_ms: input.duration_ms }),
    ...(input.trace_id === undefined ? {} : { trace_id: input.trace_id }),
    ...(input.request_ref === undefined ? {} : { request_ref: input.request_ref }),
    ...(input.provider === undefined ? {} : { provider: input.provider }),
    ...(input.model === undefined ? {} : { model: input.model }),
    ...(input.http_status === undefined ? {} : { http_status: input.http_status }),
    ...(input.item_count === undefined ? {} : { item_count: input.item_count }),
    ...(input.error_code === undefined ? {} : { error_code: input.error_code }),
    ...(input.recovery_code === undefined ? {} : { recovery_code: input.recovery_code }),
  };
  const payload = JSON.stringify(event);
  if (level === "error") {
    await error(payload);
  } else if (level === "warning") {
    await warn(payload);
  } else {
    await info(payload);
  }
}

export function logFrontendReady(): void {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return;
  }
  void logFrontendEvent("info", {
    category: "system",
    domain: "app",
    event_code: "frontend_ready",
    result: "succeeded",
    module: "frontend",
    retryable: false,
  });
}
