import { expect, test } from "@playwright/test";

/**
 * Perf probe: pump a long snake_case-heavy markdown stream through the real
 * MessageList and measure per-chunk JS cost. Run against a build and record
 * the printed JSON; compare across builds by rerunning. Not a regression
 * gate — absolute numbers depend on the machine.
 */
test("streams a long markdown doc at O(delta) per chunk", async ({ page }) => {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);

  const result = await page.evaluate(async () => {
    const { mount, tick } = window.__e2e.svelte;
    const state = window.__e2e.state;
    const sessionLib = window.__e2e.sessionLib;
    const events = window.__e2e.events;
    const { default: MessageList } = window.__e2e.MessageList;
    const target = document.createElement("div");
    target.style.height = "800px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "stream-perf-probe",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    state.streamingMessages[session.id] = [];
    mount(MessageList, { target });

    // ── Build a deterministic ~24KB document: snake_case prose, three
    // python fences, a closing table. ────────────────────────────────
    const section = (index: number) =>
      `\n\n## section_${index}_overview\n\n` +
      `The data_pipeline module exposes read_csv_file, parse_row_values, ` +
      `normalize_column_names and write_json_output. Each helper_function ` +
      `follows snake_case naming and returns a result_dict with status_code ` +
      `and error_message fields. See https://example.com/docs_v2/api_reference ` +
      `for the full_specification.\n\n` +
      "```python\n" +
      `def process_input_records_${index}(raw_records, options_dict=None):\n` +
      `    merged_options = build_default_options()\n` +
      `    if options_dict is not None:\n` +
      `        merged_options.update(options_dict)\n` +
      `    parsed_rows = [parse_row_values(row) for row in raw_records]\n` +
      `    valid_rows = [row for row in parsed_rows if row.is_valid]\n` +
      `    return {"row_count": len(valid_rows), "rows": valid_rows}\n` +
      "```\n\n" +
      `- first_item mentions retry_limit and backoff_factor\n` +
      `- second_item mentions cache_size and ttl_seconds\n\n`;
    let doc =
      "# pipeline_summary\n\nHere is the full_walkthrough of data_pipeline.";
    for (let i = 0; i < 32; i++) doc += section(i);
    doc +=
      "\n| function_name | input_params | return_value |\n" +
      "| --- | --- | --- |\n" +
      "| read_csv_file | file_path, encoding | list[dict] |\n" +
      "| parse_row_values | raw_row | parsed_row |\n" +
      "| write_json_output | records, output_path | None |\n";

    // ── Pump ~40-char chunks (cuts land anywhere: mid-fence, mid-word). ──
    const CHUNK = 40;
    const perChunk: number[] = [];
    let sent = 0;
    let index = 0;
    while (sent < doc.length) {
      const piece = doc.slice(sent, sent + CHUNK);
      const t0 = performance.now();
      events.handleEvent(session.id, `perf-${index}`, {
        model: {
          chunk: { message_id: "live-assistant", content: { text: piece } },
        },
      });
      await tick();
      perChunk.push(performance.now() - t0);
      sent += piece.length;
      index += 1;
      // Let enhancement rAFs fire periodically, like a visible browser.
      if (index % 10 === 0) await new Promise(requestAnimationFrame);
    }
    // Finalize: stream ends.
    events.handleEvent(session.id, "perf-end", {
      agent: { state_changed: { state: "idle" } },
    });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise(requestAnimationFrame);

    const sorted = [...perChunk].sort((a, b) => a - b);
    const pick = (q: number) =>
      sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
    const firstHalf = perChunk.slice(0, perChunk.length / 2);
    const secondHalf = perChunk.slice(perChunk.length / 2);
    const mean = (xs: number[]) => xs.reduce((a, b) => a + b, 0) / xs.length;

    return {
      totalChars: doc.length,
      chunks: perChunk.length,
      totalMs: Math.round(perChunk.reduce((a, b) => a + b, 0)),
      p50: +pick(0.5).toFixed(2),
      p95: +pick(0.95).toFixed(2),
      firstHalfMean: +mean(firstHalf).toFixed(2),
      secondHalfMean: +mean(secondHalf).toFixed(2),
      codeBlocks: target.querySelectorAll(".code-block").length,
      tables: target.querySelectorAll(".text-block table").length,
      italicSnake: [...target.querySelectorAll(".text-block em")].filter((el) =>
        el.textContent?.includes("_"),
      ).length,
      renderedChars: target.querySelector(".text-block")?.textContent?.length,
    };
  });

  console.log("STREAM_PERF " + JSON.stringify(result));
  expect(result.italicSnake).toBe(0);
  expect(result.codeBlocks).toBe(32);
  expect(result.tables).toBe(1);
});
