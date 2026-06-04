import { listCronJobs, deleteCronJob, triggerCronJob, updateCronJob } from "./api";

export interface CronJob {
  id: string;
  name: string;
  schedule: string;
  action: {
    ty: string;
    session_id?: string;
    content?: string;
    command?: string;
    working_dir?: string;
  };
  status: string; // "active" | "paused" | "completed" | "deleted" | "error"
  created_at: string;
  updated_at: string;
  next_run_at: string | null;
  last_run_at: string | null;
  run_count: number;
  max_runs: number | null;
  expires_at: string | null;
  last_error: string | null;
}

export class AutomationStore {
  jobs = $state<CronJob[]>([]);
  loading = $state(false);
  selectedJobId = $state<string | null>(null);
  showCreateModal = $state(false);
  editingJobId = $state<string | null>(null);
  error = $state<string | null>(null);

  get selectedJob(): CronJob | undefined {
    return this.jobs.find((j) => j.id === this.selectedJobId);
  }

  async load() {
    this.loading = true;
    this.error = null;
    try {
      const raw = await listCronJobs(undefined, 100);
      this.jobs = (raw as CronJob[]).sort(
        (a, b) =>
          new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
      );
    } catch (e: unknown) {
      this.error = e instanceof Error ? e.message : String(e);
    } finally {
      this.loading = false;
    }
  }

  async delete(jobId: string) {
    try {
      await deleteCronJob(jobId);
      this.jobs = this.jobs.filter((j) => j.id !== jobId);
      if (this.selectedJobId === jobId) this.selectedJobId = null;
    } catch (e: unknown) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  async toggleStatus(job: CronJob) {
    const newStatus = job.status === "active" ? "paused" : "active";
    try {
      await updateCronJob(job.id, { status: newStatus });
      await this.load();
    } catch (e: unknown) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  async trigger(jobId: string) {
    try {
      await triggerCronJob(jobId);
      await this.load();
    } catch (e: unknown) {
      this.error = e instanceof Error ? e.message : String(e);
    }
  }

  select(jobId: string | null) {
    this.selectedJobId = jobId;
  }

  openCreate() {
    this.editingJobId = null;
    this.showCreateModal = true;
  }

  openEdit(jobId: string) {
    this.editingJobId = jobId;
    this.showCreateModal = true;
  }

  closeModal() {
    this.showCreateModal = false;
    this.editingJobId = null;
  }
}

export const automationStore = new AutomationStore();
