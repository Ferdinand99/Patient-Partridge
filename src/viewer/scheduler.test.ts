import { describe, expect, it } from "vitest";
import { RenderScheduler, type RenderJob } from "./scheduler";

const job = (page: number, tag = ""): RenderJob => ({
  key: `${page}${tag}`,
  page,
  width: 10,
  height: 10,
});

/** Lets pending promise callbacks run. */
const settle = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};

/** A run function whose jobs finish only when the test says so. */
function controlled() {
  const started: string[] = [];
  const waiting = new Map<string, { resolve: () => void; reject: (e: unknown) => void }>();
  const run = (j: RenderJob) =>
    new Promise<void>((resolve, reject) => {
      started.push(j.key);
      waiting.set(j.key, { resolve, reject });
    });
  const finish = async (key: string) => {
    waiting.get(key)?.resolve();
    waiting.delete(key);
    await settle();
  };
  return { run, started, waiting, finish };
}

describe("RenderScheduler", () => {
  it("runs at most `concurrency` jobs at once, in priority order", async () => {
    const c = controlled();
    const s = new RenderScheduler({ concurrency: 2, run: c.run });
    s.setWanted([job(3), job(1), job(2), job(0)]);
    expect(c.started).toEqual(["3", "1"]);
    expect(s.running).toBe(2);
    expect(s.queued).toBe(2);
    await c.finish("3");
    expect(c.started).toEqual(["3", "1", "2"]);
    await c.finish("1");
    await c.finish("2");
    expect(c.started).toEqual(["3", "1", "2", "0"]);
  });

  it("drops queued work that is no longer wanted", async () => {
    const c = controlled();
    const s = new RenderScheduler({ concurrency: 1, run: c.run });
    s.setWanted([job(0), job(1), job(2)]);
    s.setWanted([job(2)]);
    await c.finish("0");
    expect(c.started).toEqual(["0", "2"]);
    await c.finish("2");
    expect(c.started).toEqual(["0", "2"]);
    expect(s.queued).toBe(0);
  });

  it("does not start a job that is already running", () => {
    const c = controlled();
    const s = new RenderScheduler({ concurrency: 2, run: c.run });
    s.setWanted([job(0)]);
    s.setWanted([job(0), job(1)]);
    expect(c.started).toEqual(["0", "1"]);
  });

  it("ignores duplicates in one list", () => {
    const c = controlled();
    const s = new RenderScheduler({ concurrency: 1, run: c.run });
    s.setWanted([job(0), job(1), job(1)]);
    expect(s.queued).toBe(1);
  });

  it("treats a changed size as new work", () => {
    const c = controlled();
    const s = new RenderScheduler({ concurrency: 2, run: c.run });
    s.setWanted([job(0, "-small")]);
    s.setWanted([job(0, "-big")]);
    expect(c.started).toEqual(["0-small", "0-big"]);
  });

  it("reports an error and keeps going", async () => {
    const c = controlled();
    const errors: unknown[] = [];
    const s = new RenderScheduler({
      concurrency: 1,
      run: c.run,
      onError: (_job, e) => errors.push(e),
    });
    s.setWanted([job(0), job(1)]);
    c.waiting.get("0")?.reject(new Error("boom"));
    await settle();
    expect(errors).toHaveLength(1);
    expect(c.started).toEqual(["0", "1"]);
  });

  it("stops taking work after stop()", () => {
    const c = controlled();
    const s = new RenderScheduler({ concurrency: 1, run: c.run });
    s.setWanted([job(0), job(1)]);
    s.stop();
    s.setWanted([job(2)]);
    expect(s.queued).toBe(0);
    expect(c.started).toEqual(["0"]);
  });
});
