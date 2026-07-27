import { readFileSync } from "node:fs";

const filePath = process.argv[2];
const requiredSections = [
  "# Codex-O 开发进度",
  "## 1. 进度总览",
  "## 2. 里程碑进度",
  "## 4. 当前任务",
  "## 11. 执行记录",
];
const requiredFields = ["状态", "改动", "原因", "验证", "进度", "下一步", "风险"];
const requiredMilestones = ["T0", "M1", "M2", "M3", "V1"];
const allowedStatuses = new Set(["待开始", "进行中", "已完成", "阻塞"]);

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

const taskRows = content
  .split("\n")
  .map((line) => line.split("|").slice(1, -1).map((cell) => cell.trim()))
  .filter(
    (row) =>
      row.length === 9 &&
      /^(?:T0|M[123]|V1)(?:-[A-Z0-9]+(?:\.\d+)*)*$/.test(row[0]),
  );
const taskIds = new Set();
let activeLeafCount = 0;

for (const row of taskRows) {
  const [taskId, parentId, , , status, , , progress] = row;

  if (taskIds.has(taskId)) {
    errors.push(`Duplicate task node: ${taskId}`);
  }
  taskIds.add(taskId);

  if (!allowedStatuses.has(status)) {
    errors.push(`${taskId}: invalid status ${status}`);
  }
  if (!/^(?:0|[1-9]\d?|100)%$/.test(progress)) {
    errors.push(`${taskId}: invalid progress ${progress}`);
  }
  if (status === "已完成" && progress !== "100%") {
    errors.push(`${taskId}: completed node must be 100%`);
  }
  if (parentId !== "--" && !taskIds.has(parentId)) {
    errors.push(`${taskId}: parent must appear before child: ${parentId}`);
  }
  if (status === "进行中" && ![...taskIds].some((id) => id.startsWith(`${taskId}-`))) {
    activeLeafCount += 1;
  }
}

for (const milestone of requiredMilestones) {
  if (!taskIds.has(milestone)) {
    errors.push(`Missing milestone node: ${milestone}`);
  }
}
if (taskRows.length < 100) {
  errors.push(`Task tree is incomplete: expected at least 100 nodes, found ${taskRows.length}`);
}
if (activeLeafCount > 1) {
  errors.push(`More than one active leaf node: ${activeLeafCount}`);
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

console.log(
  `Progress check passed: ${taskRows.length} task nodes and ${records.length} dated records validated.`,
);
