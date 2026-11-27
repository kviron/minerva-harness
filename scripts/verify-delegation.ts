import { spawnSync } from "node:child_process";
import { worktreeRoot } from "./dev-session.ts";

function run(label: string, command: string, args: string[]): number {
  console.log(`\n${label}`);
  const result = spawnSync(command, args, { cwd: worktreeRoot, stdio: "inherit" });
  if (result.error) console.error(result.error.message);
  return result.status ?? 1;
}

function verify(args: string[]): number {
  const liveCodex = args[0] === "--live-codex";
  if (args.length > Number(liveCodex)) {
    console.error("Usage: pnpm verify:delegation [--live-codex]");
    return 2;
  }
  if (liveCodex && process.platform !== "win32") {
    console.error("The live Codex checkout tests require Windows.");
    return 2;
  }
  const vendorTest = ["test", "--manifest-path", "vendor/minerva/Cargo.toml"];
  return (
    run("Delegation coordinator", "cargo", [...vendorTest, "-p", "minerva-delegation", "--lib"]) ||
    run("Isolated runtime and SQLite delegation", "cargo", [
      ...vendorTest,
      "-p",
      "minerva-runtime",
      "-p",
      "mh-sessions",
      "--lib",
      "delegation::",
    ]) ||
    run("OpenIDEA delegation adapter", "cargo", ["test", "-p", "ide-agent", "--lib", "delegation::"]) ||
    run("Chat tree and approval UI", process.execPath, [
      "node_modules/@playwright/test/cli.js",
      "test",
      "e2e/agentDelegationTree.spec.ts",
      "e2e/agentApproval.spec.ts",
      "--workers=2",
      "--timeout=60000",
    ]) ||
    (liveCodex
      ? run("Installed and authenticated Codex on Windows", "cargo", [
          "test",
          "-p",
          "ide-agent",
          "--lib",
          "live_codex_scoped_",
          "--",
          "--ignored",
          "--test-threads=1",
        ])
      : 0)
  );
}

process.exitCode = verify(process.argv.slice(2));
