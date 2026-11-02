// The environment a smoke launch runs in (see smoke.mjs).
//
// Each launch gets a profile of its own, so every run starts as a first run. On Linux the profile
// is chosen with HOME and XDG_*. On macOS HOME is the only thing the app and its web view look at.
//
// The folders named by these variables must exist. A launch whose HOME names a folder that does
// not exist hangs on some macOS machines: the window opens and takes the focus, the web view
// processes start, and the page never runs (the web view's GPU process stops answering and is
// killed over and over, without a word on any output). On the macOS runners of GitHub about one in
// eight machines does this on every launch, and the same machine starts fine as soon as HOME names
// a folder that exists. That is why the folders are created here, before the app starts.

import { mkdirSync } from "node:fs";
import { join } from "node:path";

/**
 * The environment for one launch, with the folders it names created. `dir` is a folder owned by
 * this launch. Windows keeps its profile in the places the system gives it and is left alone.
 */
export function launchEnvironment(base, dir, platform = process.platform) {
  const env = { ...base };
  if (platform === "win32") return env;
  const home = join(dir, "home");
  env.HOME = home;
  env.XDG_CONFIG_HOME = join(home, ".config");
  env.XDG_DATA_HOME = join(home, ".local", "share");
  env.XDG_CACHE_HOME = join(home, ".cache");
  for (const folder of [home, env.XDG_CONFIG_HOME, env.XDG_DATA_HOME, env.XDG_CACHE_HOME]) {
    mkdirSync(folder, { recursive: true });
  }
  return env;
}

/** How long a smoke launch may take before it counts as failed. */
export const LAUNCH_LIMIT_MS = 90_000;
/**
 * The first launch on a Windows runner starts the web view cold (its runtime and a new profile) on
 * a machine that has not run anything of it yet. That took 3 s to 74 s in CI, once over the usual
 * limit, while later launches of the same program take under 4 s. So that one launch gets more
 * time; a real regression in the app still shows in the later launches and in the numbers.
 */
export const FIRST_WINDOWS_LAUNCH_LIMIT_MS = 180_000;

/** The time limit of launch number `index` (0 for the first one). */
export function launchLimitMs(index, platform = process.platform) {
  return platform === "win32" && index === 0 ? FIRST_WINDOWS_LAUNCH_LIMIT_MS : LAUNCH_LIMIT_MS;
}
