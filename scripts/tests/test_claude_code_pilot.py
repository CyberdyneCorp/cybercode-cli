"""Safety and dry-run logic of the Cyber vs Claude Code pilot runner. No model is called."""

import argparse
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import claude_code_pilot as pilot

SECRET = "sk-ant-test-0123456789abcdef"


def config():
    return pilot.load_config(pilot.DEFAULT_CONFIG)


def binaries(root: Path) -> dict:
    return {
        "cyber": root / "cyber", "sandbox_helper": root / "cyber-sandbox-exec", "claude": root / "claude",
        "python3": Path("/usr/bin/python3"), "node": Path("/usr/bin/node"), "git": Path("/usr/bin/git"), "bash": Path("/bin/bash"),
    }


class Key(unittest.TestCase):
    def test_environment_wins_and_is_reported_without_value(self):
        key, source = pilot.load_api_key({pilot.KEY_VAR: SECRET, pilot.ENV_FILE_VAR: "/nonexistent"})
        self.assertEqual((key, source), (SECRET, "env"))
        check = pilot.check_key({pilot.KEY_VAR: SECRET})
        self.assertTrue(check.ok)
        self.assertNotIn(SECRET, check.line())

    def test_env_file_is_parsed_and_must_be_private(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "eval.env"
            path.write_text(f"OTHER=1\nexport {pilot.KEY_VAR}=\"{SECRET}\"\n")
            path.chmod(0o600)
            self.assertEqual(pilot.load_api_key({pilot.ENV_FILE_VAR: str(path)}), (SECRET, "file"))
            if os.name == "posix":
                path.chmod(0o644)
                with self.assertRaisesRegex(pilot.PilotError, "chmod 600") as caught:
                    pilot.load_api_key({pilot.ENV_FILE_VAR: str(path)})
                self.assertNotIn(SECRET, str(caught.exception))

    def test_missing_key_is_a_dry_run_warning(self):
        self.assertEqual(pilot.load_api_key({}), (None, "missing"))
        check = pilot.check_key({})
        self.assertFalse(check.ok)
        self.assertTrue(check.warning)

    def test_scrub_replaces_key_in_trial_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "a" / "stderr.log"
            log.parent.mkdir()
            log.write_text(f"auth {SECRET} failed")
            self.assertEqual(pilot.scrub_tree(Path(tmp), SECRET), 1)
            self.assertNotIn(SECRET, log.read_text())
            self.assertIn(pilot.REDACTED, log.read_text())


class SpendGate(unittest.TestCase):
    def test_paid_modes_require_confirmation_and_key(self):
        args = argparse.Namespace(confirm_spend=False)
        with self.assertRaisesRegex(pilot.PilotError, "--confirm-spend"):
            pilot.require_spend_confirmation(args, SECRET)
        with self.assertRaisesRegex(pilot.PilotError, pilot.KEY_VAR):
            pilot.require_spend_confirmation(argparse.Namespace(confirm_spend=True), None)
        pilot.require_spend_confirmation(argparse.Namespace(confirm_spend=True), SECRET)

    def test_run_without_confirmation_starts_no_process(self):
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.dict(os.environ, {pilot.KEY_VAR: SECRET}), \
                mock.patch("subprocess.Popen") as popen, mock.patch("subprocess.run") as run:
            status = pilot.main(["--run", "--root", tmp])
            self.assertEqual(status, 2)
            popen.assert_not_called()
            run.assert_not_called()

    def test_smoke_without_key_starts_no_process(self):
        env = {k: v for k, v in os.environ.items() if k not in (pilot.KEY_VAR, pilot.ENV_FILE_VAR)}
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, env, clear=True), \
                mock.patch("subprocess.Popen") as popen:
            self.assertEqual(pilot.main(["--smoke", "--confirm-spend", "--root", tmp]), 2)
            popen.assert_not_called()

    def test_spend_guard_stops_scheduling(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg, layout = config(), pilot.Layout(Path(tmp))
            tasks = pilot.load_tasks(cfg)[:1]
            ctx = pilot.RunContext(cfg, layout, binaries(Path(tmp)), SECRET, "scored", max_total_usd=1.0)
            ctx.results.append({"native_cost_usd": 1.5, "computed_cost_usd": 1.4})
            with mock.patch.object(pilot, "run_claude_trial") as claude, mock.patch.object(pilot, "run_cyber_trial") as cyber:
                self.assertEqual(pilot.execute(ctx, tasks, pilot.frozen_plan(tasks)), 3)
                claude.assert_not_called()
                cyber.assert_not_called()


class Execution(unittest.TestCase):
    def record(self, harness, infra=False):
        return {"harness": harness, "passed": not infra, "infrastructure_failure": infra,
                "latency_seconds": 1.0, "native_cost_usd": 0.1, "computed_cost_usd": 0.1}

    def test_early_infrastructure_failure_stops_and_resume_skips_recorded_trials(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg, layout = config(), pilot.Layout(Path(tmp))
            tasks = pilot.load_tasks(cfg)[:2]
            plan = pilot.frozen_plan(tasks)
            ctx = pilot.RunContext(cfg, layout, binaries(Path(tmp)), SECRET, "scored", max_total_usd=100)
            with mock.patch.object(pilot, "run_cyber_trial", return_value=self.record("cyber", infra=True)) as cyber, \
                    mock.patch.object(pilot, "run_claude_trial", return_value=self.record("claude-code")) as claude:
                self.assertEqual(pilot.execute(ctx, tasks, plan), 2)
                self.assertEqual(cyber.call_count, 1)
                claude.assert_not_called()
            ctx = pilot.RunContext(cfg, layout, binaries(Path(tmp)), SECRET, "scored", max_total_usd=100)
            with mock.patch.object(pilot, "run_cyber_trial", return_value=self.record("cyber")) as cyber, \
                    mock.patch.object(pilot, "run_claude_trial", return_value=self.record("claude-code")) as claude:
                self.assertEqual(pilot.execute(ctx, tasks, plan), 0)
                self.assertEqual(cyber.call_count, 1)  # the first task's Cyber trial was resumed
                self.assertEqual(claude.call_count, 2)
            self.assertEqual(len(ctx.results), 4)


class Isolation(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.cfg = config()
        self.layout = pilot.Layout(self.root)
        self.bins = binaries(self.root)
        self.task = pilot.load_tasks(self.cfg)[0]

    def tearDown(self):
        self.tmp.cleanup()

    def test_claude_env_is_fresh_and_pins_every_model_alias(self):
        with mock.patch.dict(os.environ, {"LEAK_ME": "1", "CLAUDE_CONFIG_DIR": str(Path.home() / ".claude")}):
            env = pilot.claude_env(self.cfg, self.layout, self.bins, self.root / "t", SECRET)
        self.assertNotIn("LEAK_ME", env)
        self.assertEqual(env["CLAUDE_CONFIG_DIR"], str(self.layout.claude_config_dir))
        self.assertNotEqual(Path(env["HOME"]), Path.home())
        for name in self.cfg["claude_code"]["model_pin_env"]:
            self.assertEqual(env[name], self.cfg["model"]["id"])
        self.assertEqual(env["CLAUDE_CODE_EFFORT_LEVEL"], self.cfg["model"]["effort"])
        self.assertEqual(pilot.public_env(env)[pilot.KEY_VAR], pilot.REDACTED)

    def test_graders_never_receive_the_key(self):
        env = pilot.grader_env(self.layout, self.bins, self.root / "t")
        self.assertNotIn(pilot.KEY_VAR, env)
        self.assertNotIn(SECRET, json.dumps(env))

    def test_claude_command_flags(self):
        cmd = pilot.claude_command(self.cfg, self.bins, self.layout, self.task)
        self.assertEqual(cmd[-1], self.task.prompt)
        self.assertNotIn(SECRET, " ".join(cmd))
        self.assertEqual(cmd[cmd.index("--model") + 1], self.cfg["model"]["id"])
        self.assertEqual(cmd[cmd.index("--effort") + 1], self.cfg["model"]["effort"])
        self.assertEqual(cmd[cmd.index("--permission-prompts") + 1], "none")
        self.assertEqual(cmd[cmd.index("--max-budget-usd") + 1], f"{self.task.budget_usd:g}")
        # The variadic tool list must be closed by another option before the prompt.
        self.assertLess(cmd.index("--disallowedTools"), cmd.index("--max-budget-usd"))
        self.assertIn("--strict-mcp-config", cmd)
        self.assertNotIn("--dangerously-skip-permissions", cmd)

    def test_cyber_command_runs_one_trial_of_one_task(self):
        cmd = pilot.cyber_command(self.cfg, self.bins, self.task, self.root / "out")
        self.assertEqual(cmd[cmd.index("--task") + 1], self.task.id)
        self.assertEqual(cmd[cmd.index("--trials") + 1], "1")
        self.assertEqual(cmd[cmd.index("--model") + 1], self.cfg["model"]["cyber_ref"])

    def test_generated_settings_disable_command_network_and_hide_private_paths(self):
        pilot.write_isolated_config(self.cfg, self.layout)
        settings = json.loads(self.layout.claude_settings.read_text())
        self.assertEqual(settings["sandbox"]["network"], {"allowedDomains": [], "strictAllowlist": True})
        self.assertFalse(settings["sandbox"]["allowUnsandboxedCommands"])
        denied = settings["sandbox"]["filesystem"]["denyRead"]
        self.assertIn(str(pilot.ROOT / "eval" / "graders"), denied)
        self.assertIn(str(Path.home() / ".claude"), denied)
        self.assertIn(str(self.root / "scored"), denied)
        cyber = json.loads((self.layout.cyber_config_dir / "cyber.json").read_text())
        self.assertEqual(cyber["sandbox"]["network"], "off")
        self.assertEqual(cyber["model_roles"]["small"], self.cfg["model"]["cyber_ref"])
        self.assertIn(f"{pilot.ROOT / 'eval' / 'graders'}/**", cyber["sandbox"]["deny_read"])
        self.assertIn(f"{self.root / 'claude-code'}/**", cyber["sandbox"]["deny_read"])

    def test_isolation_checks_pass_for_a_private_root_and_fail_inside_the_repository(self):
        pilot.write_isolated_config(self.cfg, self.layout)
        failed = [c for c in pilot.check_isolation(self.cfg, self.layout) if not c.ok and not c.warning]
        self.assertEqual(failed, [])
        inside = pilot.Layout(pilot.ROOT / "eval" / "pilots" / "run")
        checks = {c.name: c.ok for c in pilot.check_run_root(inside)}
        self.assertFalse(checks["run root outside repository"])


class Outcomes(unittest.TestCase):
    def test_classification(self):
        self.assertEqual(pilot.classify_claude(0, False, {"subtype": "success", "is_error": False}), (False, "end_turn"))
        self.assertEqual(pilot.classify_claude(1, False, {"subtype": "error_max_budget_usd"}), (False, "budget"))
        self.assertEqual(pilot.classify_claude(None, True, None), (False, "timeout"))
        self.assertEqual(pilot.classify_claude(1, False, None), (True, "error"))
        self.assertEqual(pilot.classify_claude(1, False, {"subtype": "error_during_execution", "is_error": True}),
                         (True, "error_during_execution"))

    def test_usage_sums_every_model_and_lists_them(self):
        tokens, models = pilot.claude_usage({"modelUsage": {
            "claude-fable-5-2": {"inputTokens": 10, "outputTokens": 20, "cacheReadInputTokens": 30, "cacheCreationInputTokens": 40},
            "claude-haiku-5-5": {"inputTokens": 1, "outputTokens": 2, "cacheReadInputTokens": 3, "cacheCreationInputTokens": 4},
        }})
        self.assertEqual(tokens, {"input": 11, "output": 22, "cache_read": 33, "cache_write": 44})
        self.assertEqual(models, ["claude-fable-5-2", "claude-haiku-5-5"])

    def test_plan_is_frozen_and_alternates_harness_order(self):
        tasks = pilot.load_tasks(config())
        plan = pilot.frozen_plan(tasks)
        self.assertEqual(len(plan), 40)
        self.assertEqual(plan[0]["order"], ["cyber", "claude-code"])
        self.assertEqual(plan[1]["order"], ["claude-code", "cyber"])
        self.assertEqual(plan, pilot.frozen_plan(tasks))

    def test_estimate_arithmetic(self):
        cfg = config()
        est = pilot.estimate(cfg, pilot.load_tasks(cfg))
        prices = cfg["model"]["prices_usd_per_mtok"]
        hard = est["hard_per_trial_tokens"]
        self.assertAlmostEqual(est["hard_per_trial_usd"], sum(hard[k] * prices[k] for k in pilot.TOKEN_CLASSES) / 1e6)
        self.assertAlmostEqual(est["both_harnesses_usd"], 2 * est["per_harness_usd"])
        self.assertAlmostEqual(est["budget_cap_per_harness_usd"], 68.0)
        self.assertLess(est["both_harnesses_upper_usd"], est["budget_cap_both_usd"])


if __name__ == "__main__":
    unittest.main()
