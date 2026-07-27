import { readFileSync } from "node:fs";

const filePath = process.argv[2];
const requiredSections = [
  "# Codex-O 开发进度",
  "## 1. 进度总览",
  "## 2. 里程碑进度",
  "## 4. 当前任务",
  "## 9. 逐步变更记录",
];
const requiredFields = ["状态", "改动", "原因", "验证", "进度", "下一步", "风险"];

if (!filePath) {
  console.error("Usage: node scripts/check-progress.mjs <PROGRESS.md>");
  process.exit(1);
}

let source;
try {
  source = readFileSync(filePath, "utf8");
} catch (error) {
  console.error(`Cannot read progress file: ${error.message}`);
  process.exit(1);
}

const errors = [];
const content = source.replace(/```[\s\S]*?```/g, "");

for (const section of requiredSections) {
  if (!content.includes(section)) {
    errors.push(`Missing required section: ${section}`);
  }
}

const records = content.match(/^### \d{4}-\d{2}-\d{2} \d{2}:\d{2} - .+$/gm) ?? [];
if (records.length === 0) {
  errors.push("No dated change record found.");
}

for (const heading of records) {
  const start = content.indexOf(heading);
  const nextRecord = content.indexOf("\n### ", start + heading.length);
  const record = content.slice(start, nextRecord === -1 ? content.length : nextRecord);

  for (const field of requiredFields) {
    if (!new RegExp(`^- ${field}：.+$`, "m").test(record)) {
      errors.push(`${heading}: missing field ${field}`);
    }
  }
}

if (errors.length > 0) {
  console.error("Progress check failed:");
  for (const error of errors) {
    console.error(`- ${error}`);
  }
  process.exit(1);
}

console.log(`Progress check passed: ${records.length} dated records validated.`);
