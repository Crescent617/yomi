import { basicSetup, EditorView } from "codemirror";
import { keymap } from "@codemirror/view";

import { type Extension } from "@codemirror/state";
import { appSyntaxHighlighting, appTheme } from "./theme";

const langCache = new Map<string, Extension>();

export async function loadLanguage(
  filename: string,
): Promise<Extension | null> {
  const ext = filename.split(".").pop()?.toLowerCase();
  if (!ext) return null;

  if (langCache.has(ext)) return langCache.get(ext) ?? null;

  // TOML has no Lezer grammar; the official approach is the ported
  // stream mode from @codemirror/legacy-modes.
  if (ext === "toml") {
    const [{ StreamLanguage }, { toml }] = await Promise.all([
      import("@codemirror/language"),
      import("@codemirror/legacy-modes/mode/toml"),
    ]);
    const lang = StreamLanguage.define(toml);
    langCache.set(ext, lang);
    return lang;
  }

  let mod: { [key: string]: () => Extension };
  switch (ext) {
    case "rs":
      mod = (await import("@codemirror/lang-rust")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "js":
    case "jsx":
      mod = (await import("@codemirror/lang-javascript")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "ts":
    case "tsx":
      mod = (await import("@codemirror/lang-javascript")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "py":
      mod = (await import("@codemirror/lang-python")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "go":
      mod = (await import("@codemirror/lang-go")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "java":
      mod = (await import("@codemirror/lang-java")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "json":
      mod = (await import("@codemirror/lang-json")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "md":
      mod = (await import("@codemirror/lang-markdown")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "html":
      mod = (await import("@codemirror/lang-html")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "css":
    case "scss":
      mod = (await import("@codemirror/lang-css")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "sql":
      mod = (await import("@codemirror/lang-sql")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    case "yaml":
    case "yml":
      mod = (await import("@codemirror/lang-yaml")) as unknown as {
        [key: string]: () => Extension;
      };
      break;
    default:
      return null;
  }

  const lang = mod[Object.keys(mod)[0]]?.();
  if (lang) langCache.set(ext, lang);
  return lang;
}

export interface EditorConfig {
  doc: string;
  filename: string;
  onChange?: (value: string) => void;
  onSave?: (value: string) => void;
  onKeymap?: Record<string, (view: EditorView) => boolean>;
  extensions?: Extension[];
}

export async function createEditor(
  parent: HTMLElement,
  config: EditorConfig,
): Promise<EditorView> {
  // Chrome and syntax colors come from the app's semantic CSS variables
  // (editor/theme.ts), so editors follow light/dark and custom palettes.
  const extensions = [
    basicSetup,
    appTheme,
    appSyntaxHighlighting,
    ...(config.extensions ?? []),
  ];

  const lang = await loadLanguage(config.filename);
  if (lang) extensions.push(lang);

  if (config.onChange) {
    extensions.push(
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          config.onChange!(update.state.doc.toString());
        }
      }),
    );
  }

  const customKeymap: Array<{
    key: string;
    run: (view: EditorView) => boolean;
  }> = [];
  if (config.onSave) {
    customKeymap.push({
      key: "Ctrl-s",
      run: (view: EditorView) => {
        config.onSave!(view.state.doc.toString());
        return true;
      },
    });
    customKeymap.push({
      key: "Mod-s",
      run: (view: EditorView) => {
        config.onSave!(view.state.doc.toString());
        return true;
      },
    });
  }

  if (customKeymap.length > 0) {
    extensions.push(keymap.of(customKeymap));
  }

  return new EditorView({
    doc: config.doc,
    parent,
    extensions,
  });
}
