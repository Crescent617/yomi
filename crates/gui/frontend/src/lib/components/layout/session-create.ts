export interface SessionCreationSource {
  project_id: string | null;
  working_dir: string | null;
  auto_approve_level: string | null;
  model_key: string | null;
}

export interface CreateFromSessionParams {
  project_id?: string;
  working_dir: string;
  permission_level: string;
  model_key?: string;
}

/** Select only the basic runtime parameters copied by "Create from". */
export function createFromSessionParams(
  source: SessionCreationSource,
  projectDir?: string,
): CreateFromSessionParams {
  return {
    project_id: source.project_id ?? undefined,
    working_dir: projectDir ?? source.working_dir ?? "",
    permission_level: source.auto_approve_level ?? "caution",
    model_key: source.model_key ?? undefined,
  };
}
