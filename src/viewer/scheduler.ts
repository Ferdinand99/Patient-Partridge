// A small queue for page renders: a couple at a time, most important first, and work that is
// no longer wanted (the user scrolled on) is dropped before it starts.

export interface RenderJob {
  /** Identifies the work: the same key is never run twice at once. */
  key: string;
  page: number;
  width: number;
  height: number;
}

export interface SchedulerOptions {
  /** How many renders run at the same time. */
  concurrency: number;
  run: (job: RenderJob) => Promise<void>;
  onError?: (job: RenderJob, error: unknown) => void;
}

export class RenderScheduler {
  private queue: RenderJob[] = [];
  private inFlight = new Set<string>();
  private stopped = false;

  constructor(private readonly options: SchedulerOptions) {}

  /**
   * Replaces the wanted work. `jobs` is in priority order, most important first. Jobs that are
   * already running are left alone; queued jobs that are not on the list any more are dropped.
   */
  setWanted(jobs: RenderJob[]): void {
    if (this.stopped) return;
    const seen = new Set<string>();
    this.queue = jobs.filter((job) => {
      if (this.inFlight.has(job.key) || seen.has(job.key)) return false;
      seen.add(job.key);
      return true;
    });
    this.pump();
  }

  /** Forgets everything queued and refuses new work. Running renders finish on their own. */
  stop(): void {
    this.stopped = true;
    this.queue = [];
  }

  get queued(): number {
    return this.queue.length;
  }

  get running(): number {
    return this.inFlight.size;
  }

  private pump(): void {
    while (this.inFlight.size < this.options.concurrency && this.queue.length > 0) {
      const job = this.queue.shift() as RenderJob;
      this.inFlight.add(job.key);
      void this.start(job);
    }
  }

  private async start(job: RenderJob): Promise<void> {
    try {
      await this.options.run(job);
    } catch (error) {
      this.options.onError?.(job, error);
    } finally {
      this.inFlight.delete(job.key);
      this.pump();
    }
  }
}
