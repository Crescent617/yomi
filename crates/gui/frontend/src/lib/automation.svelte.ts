import {
  listCronJobs,
  deleteCronJob,
  triggerCronJob,
  updateCronJob,
  errorMessage,
  type CronJob,
} from "./api";
import { sendDesktopNotification } from "./state.svelte";

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
      const jobs = await listCronJobs(undefined, 100);
      this.jobs = jobs.sort(
        (a, b) =>
          new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime(),
      );
    } catch (e: unknown) {
      this.error = errorMessage(e);
    } finally {
      this.loading = false;
    }
  }

  async delete(job_id: string) {
    try {
      await deleteCronJob(job_id);
      this.jobs = this.jobs.filter((j) => j.id !== job_id);
      if (this.selectedJobId === job_id) this.selectedJobId = null;
    } catch (e: unknown) {
      this.error = errorMessage(e);
    }
  }

  async toggleStatus(job: CronJob) {
    const newStatus = job.status === "active" ? "paused" : "active";
    try {
      await updateCronJob(job.id, { status: newStatus });
      await this.load();
    } catch (e: unknown) {
      this.error = errorMessage(e);
    }
  }

  async trigger(job_id: string) {
    try {
      await triggerCronJob(job_id);
      await this.load();
      const job = this.jobs.find((j) => j.id === job_id);
      const session_id = job?.action?.session_id;
      sendDesktopNotification(
        "Yomi",
        `Task "${job?.name ?? job_id}" completed`,
        session_id,
      );
    } catch (e: unknown) {
      this.error = errorMessage(e);
      const job = this.jobs.find((j) => j.id === job_id);
      const session_id = job?.action?.session_id;
      sendDesktopNotification(
        "Yomi",
        `Task "${job_id}" failed: ${this.error}`,
        session_id,
      );
    }
  }

  select(job_id: string | null) {
    this.selectedJobId = job_id;
  }

  openCreate() {
    this.editingJobId = null;
    this.showCreateModal = true;
  }

  openEdit(job_id: string) {
    this.editingJobId = job_id;
    this.showCreateModal = true;
  }

  closeModal() {
    this.showCreateModal = false;
    this.editingJobId = null;
  }
}

export const automationStore = new AutomationStore();
