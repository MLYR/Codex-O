import { readFileSync } from "node:fs";

const reportPath =
  process.argv[2] ?? "src-tauri/target/m1-beta/m1-gate.json";
const evaluationPath =
  process.argv[3] ?? "src-tauri/m1-beta-evaluation.json";

const errors = [];
const report = readJson(reportPath, "M1 Gate report");
const evaluation = readJson(evaluationPath, "M1 Beta evaluation");
const libSource = readFileSync("src-tauri/src/lib.rs", "utf8");
const packageJson = JSON.parse(readFileSync("package.json", "utf8"));

check(report?.version === 1, "Gate report version must be 1");
check(report?.offlineBeta === true, "Gate must be labelled offline Beta");
check(report?.performance?.skillCount === 200, "Performance fixture must contain 200 Skills");
check(report?.performance?.listP95Ms < 3000, "Cached list p95 must be below 3 seconds");
check(
  report?.performance?.passportP95Ms < 3000,
  "Cached passport p95 must be below 3 seconds",
);
check(report?.parsing?.coverage >= 0.95, "Parsing coverage must be at least 95%");
check(report?.parsing?.damagedIsolated === true, "Damaged Skills must be isolated");
check(
  report?.identity?.sameNameDistinctIds === true,
  "Same-name Skills must keep distinct stable IDs",
);
check(
  report?.identity?.providerDisambiguated === true,
  "Same-name Skills must retain Provider disambiguation",
);
check(
  report?.degradation?.availability === 1,
  "Static list and detail availability must be 100% in degraded scenarios",
);
check(
  report?.capabilities?.writeCapabilitiesFalse === true,
  "Read-only Provider write capabilities must all be false",
);
check(report?.safety?.dtoLeakCount === 0, "Safety DTO leak count must be zero");
check(report?.safety?.logLeakCount === 0, "Log leak count must be zero");
check(report?.safety?.progressLeakCount === 0, "Progress leak count must be zero");
check(report?.schema?.cases === 100, "Schema corpus must contain 100 cases");
check(report?.schema?.strictRate >= 0.99, "Strict SkillPassport rate must be at least 99%");
check(
  report?.schema?.rejectionRate === 1,
  "Expected invalid output rejection rate must be 100%",
);
check(
  report?.schema?.credentialResiduals === 0,
  "Raw credential residual count must be zero",
);
check(
  report?.schema?.invalidEvidenceAccepted === 0,
  "Out-of-range EvidenceRef acceptance must be zero",
);
check(report?.schema?.protocols?.length === 3, "All three AI protocols must be covered");
check(
  report?.schema?.uniquePassportBodies > 50,
  "Schema corpus must not reuse one static passport body",
);
check(
  report?.betaFixtures?.abnormalCases >= 10 &&
    report?.betaFixtures?.abnormalIsolated === report?.betaFixtures?.abnormalCases,
  "At least 10 abnormal fixtures must be isolated",
);
check(
  report?.betaFixtures?.securityCases >= 10 &&
    report?.betaFixtures?.securityResiduals === 0,
  "At least 10 security fixtures must have zero raw residuals",
);
check(report?.realBeta?.sampleCount >= 10, "Real Beta must include at least 10 samples");
check(
  report?.realBeta?.parseableCount >= 10,
  "Real Beta must include at least 10 parseable samples",
);

const reportText = JSON.stringify(report);
for (const marker of ["/Users/", "\\Users\\", ".agents/skills", ".codex/skills", "BEGIN PRIVATE KEY"]) {
  check(!reportText.includes(marker), `Gate report contains forbidden marker: ${marker}`);
}
check(
  report?.realBeta?.samples?.every(
    (sample) =>
      /^sample-\d{2}$/.test(sample.sampleId) &&
      ["small", "medium", "large"].includes(sample.sizeBand) &&
      Array.isArray(sample.diagnosticCodes),
  ),
  "Real Beta metadata must use anonymous IDs, size bands, and diagnostic codes only",
);

const commandInventory = extractCommandInventory(libSource);
const forbiddenSkillWrites = commandInventory.filter((command) =>
  /(?:install|quarantine|restore|update|delete).*skill|skill.*(?:install|quarantine|restore|update|delete)/i.test(
    command,
  ),
);
check(
  forbiddenSkillWrites.length === 0,
  `Forbidden Skill write commands found: ${forbiddenSkillWrites.join(", ")}`,
);
check(
  commandInventory.length >= 10,
  "Tauri command inventory could not be extracted reliably",
);
check(
  packageJson.scripts?.["check:m1"]?.includes("check-m1-gate.mjs"),
  "npm run check:m1 must invoke the Node Gate checker",
);

check(evaluation?.version === 1, "Beta evaluation version must be 1");
check(evaluation?.blind === true, "Beta evaluation must be blind");
check(evaluation?.providerHidden === true, "Blind evaluator must not receive Provider identity");
check(evaluation?.generatorHidden === true, "Blind evaluator must not receive generator identity");
check(evaluation?.sampleIds?.length >= 10, "Blind evaluation must cover at least 10 samples");
const bindingChecks = evaluation?.bindingChecks ?? [];
const validBindings = bindingChecks.filter((binding) => binding.valid === true).length;
const bindingSupportRate =
  bindingChecks.length === 0 ? 0 : validBindings / bindingChecks.length;
check(bindingChecks.length >= 50, "Blind evaluation must check at least 50 evidence bindings");
check(
  new Set(bindingChecks.map((binding) => binding.claimId)).size === bindingChecks.length,
  "Evidence binding claim IDs must be unique",
);
check(
  bindingChecks.every(
    (binding) =>
      /^sample-\d{2}-(?:purpose|trigger|prerequisites|distinction|risk)$/.test(
        binding.claimId,
      ) &&
      ["evidence", "uncertainty"].includes(binding.resolution),
  ),
  "Evidence bindings must use anonymous fixed-category claim IDs",
);
check(evaluation?.claims?.total === bindingChecks.length, "Claim total must match binding checks");
check(evaluation?.claims?.supported === validBindings, "Supported total must match binding checks");
check(
  Math.abs((evaluation?.claims?.supportRate ?? 0) - bindingSupportRate) < 0.000001,
  "Support rate must be derived from binding checks",
);
check(bindingSupportRate >= 0.9, "Evidence support rate must be at least 90%");
check(
  evaluation?.claims?.unsupportedRecordedAsUncertainty === true,
  "Unsupported conclusions must be recorded as uncertainties",
);
for (const key of ["purpose", "trigger", "prerequisites", "distinction", "risk"]) {
  check(evaluation?.scores?.[key] >= 4, `${key} score must be at least 4.0/5`);
}
check(
  evaluation?.sampleIds?.every((sampleId) => /^sample-\d{2}$/.test(sampleId)),
  "Evaluation may contain only anonymous sample IDs",
);

if (errors.length > 0) {
  console.error("M1 Gate failed:");
  for (const error of errors) {
    console.error(`- ${error}`);
  }
  process.exit(1);
}

console.log(
  [
    "M1 Gate passed",
    `list_p95=${report.performance.listP95Ms.toFixed(2)}ms`,
    `passport_p95=${report.performance.passportP95Ms.toFixed(2)}ms`,
    `parse=${percent(report.parsing.coverage)}`,
    `schema=${percent(report.schema.strictRate)}`,
    `evidence=${percent(bindingSupportRate)}`,
    `commands=${commandInventory.length}`,
    `real_samples=${report.realBeta.sampleCount}`,
  ].join(" "),
);

function readJson(path, label) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    errors.push(`${label} unavailable at ${path}: ${error.message}`);
    return null;
  }
}

function check(condition, message) {
  if (!condition) {
    errors.push(message);
  }
}

function extractCommandInventory(source) {
  const match = source.match(/tauri::generate_handler!\s*\[([\s\S]*?)\]/);
  if (!match) {
    return [];
  }
  return match[1]
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean)
    .map((value) => value.split("::").at(-1));
}

function percent(value) {
  return `${(value * 100).toFixed(1)}%`;
}
