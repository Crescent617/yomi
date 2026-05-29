import { basicSetup, EditorView } from "codemirror";
import { oneDark } from "@codemirror/theme-one-dark";
import { keymap } from "@codemirror/view";

const langCache = new Map<string, any>();

export async function loadLanguage(filename: string): Promise<any> {
  const ext = filename.split(".").pop()?.toLowerCase();
  if (!ext) return null;

  if (langCache.has(ext)) return langCache.get(ext);

  let mod: any;
  switch (ext) {
    case "rs":
      mod = await import("@codemirror/lang-rust");
      break;
    case "js":
    case "jsx":
      mod = await import("@codemirror/lang-javascript");
      break;
    case "ts":
    case "tsx":
      mod = await import("@codemirror/lang-javascript");
      break;
    case "py":
      mod = await import("@codemirror/lang-python");
      break;
    case "go":
      mod = await import("@codemirror/lang-go");
      break;
    case "java":
      mod = await import("@codemirror/lang-java");
      break;
    case "json":
      mod = await import("@codemirror/lang-json");
      break;
    case "md":
      mod = await import("@codemirror/lang-markdown");
      break;
    case "html":
      mod = await import("@codemirror/lang-html");
      break;
    case "css":
    case "scss":
      mod = await import("@codemirror/lang-css");
      break;
    case "sql":
      mod = await import("@codemirror/lang-sql");
      break;
    case "yaml":
    case "yml":
      mod = await import("@codemirror/lang-yaml");
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
  theme: "light" | "dark";
  onChange?: (value: string) => void;
  onSave?: (value: string) => void;
  onKeymap?: Record<string, (view: EditorView) => boolean>;
}

export async function createEditor(
  parent: HTMLElement,
  config: EditorConfig
): Promise<EditorView> {
  const extensions = [basicSetup];

  const lang = await loadLanguage(config.filename);
  if (lang) extensions.push(lang);

  if (config.theme === "dark") {
    extensions.push(oneDark);
  }

  if (config.onChange) {
    extensions.push(
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          config.onChange!(update.state.doc.toString());
        }
      })
    );
  }

  const customKeymap: any[] = [];
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
