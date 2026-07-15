import { describe, expect, test } from "vitest";
import { createFromSessionParams } from "./session-create";

describe("createFromSessionParams", () => {
  test("copies project, model and approval level", () => {
    expect(
      createFromSessionParams(
        {
          project_id: "project_1",
          working_dir: "/stale/path",
          auto_approve_level: "full",
          model_key: "openai/gpt-5",
        },
        "/current/project",
      ),
    ).toEqual({
      project_id: "project_1",
      working_dir: "/current/project",
      permission_level: "full",
      model_key: "openai/gpt-5",
    });
  });

  test("uses safe fallbacks for nullable backend fields", () => {
    expect(
      createFromSessionParams({
        project_id: null,
        working_dir: null,
        auto_approve_level: null,
        model_key: null,
      }),
    ).toEqual({
      project_id: undefined,
      working_dir: "",
      permission_level: "caution",
      model_key: undefined,
    });
  });
});
