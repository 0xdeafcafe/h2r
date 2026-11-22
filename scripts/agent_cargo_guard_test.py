#!/usr/bin/env python3
"""Tests for scripts/agent-cargo-guard.py: python3 scripts/agent_cargo_guard_test.py"""

import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location(
    "guard", pathlib.Path(__file__).with_name("agent-cargo-guard.py")
)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class Guard(unittest.TestCase):
    def blocked(self, command):
        self.assertIsNotNone(guard.check(command), command)

    def allowed(self, command):
        self.assertIsNone(guard.check(command), command)

    def test_tests_go_through_xtask(self):
        self.blocked("cargo test -p game")
        self.blocked("cd x && cargo test -q -p cli --bin h2 lab::")
        self.blocked("cargo +nightly test")
        self.blocked("cargo nextest run -p bots")
        self.allowed("cargo xtask test -p game -p bots")
        self.allowed("cargo xtask test")

    def test_install_is_refused(self):
        self.blocked("cargo install -q --path crates/apps/cli --locked")
        self.allowed("cargo h2 play disc/halo2.iso")

    def test_cache_overrides_are_refused(self):
        self.blocked("RUSTC_WRAPPER= cargo build")
        self.blocked("export CARGO_TARGET_DIR=/tmp/t; cargo check")
        self.blocked("SCCACHE_DIR=x cargo check")
        self.allowed("H2_OPT=2 H2_OBJ_CACHE=~/Library/Caches/h2recomp/obj cargo h2 build d")

    def test_the_worktree_is_a_leading_cd_or_the_cwd(self):
        self.assertEqual(guard.worktree_dir("cargo check -p a", "/w"), "/w")
        self.assertEqual(guard.worktree_dir("cd /x/y && cargo check", "/w"), "/x/y")
        self.assertEqual(guard.worktree_dir("cd sub; cargo check", "/w"), "/w/sub")
        self.assertIsNotNone(guard.CARGO.search("nice -n 10 cargo check -q"))
        self.assertIsNone(guard.CARGO.search("rg cargo-guard docs"))

    def test_other_commands_pass(self):
        self.allowed("cargo check -p game --tests")
        self.allowed("cargo clippy -q -p game --all-targets -- -D warnings")
        self.allowed("git log --oneline -3")
        self.allowed("grep -n 'cargo test' docs/dev/build-speed.md")


if __name__ == "__main__":
    unittest.main()
