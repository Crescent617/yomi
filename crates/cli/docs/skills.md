# skills

skill = 一个目录 + 其中的 `SKILL.md`：frontmatter 声明用途，正文写操作约定。agent 的 system prompt 里带 skill 索引（name + description + path），任务命中 description 时用 `read` 加载正文。

## 安装

安装 = 把目录放进任一 skill 目录，无 CLI 命令。三层目录，优先级低 → 高（同名高层胜出）：

| 层 | 路径 | 适用 |
|---|---|---|
| 全局 | `~/.agents/skills` | 跨工具共享（其他 agent 工具也读这层） |
| 数据目录 | `<data_dir>/skills`（默认 `~/.yomi/skills`） | yomi 专有，随数据目录走 |
| 工作区 | `<工作目录>/.agents/skills` | 项目/工作区专用，目录不存在则不参与扫描 |

符号链接会被跟随（stow/nix 部署可用），dangling 链接跳过。

生效时机：spawn 时经 loader 加载，目录扫描结果缓存 60s（全 daemon 共享）——新装的 skill 最晚约 1 分钟内生效，无 reload 命令。`yomi skill list` 查看当前生效清单（同样走缓存）。

## 从生态安装别人的 skill

- 社区安装 CLI（**裸跑是交互式的**，非交互形式）：

  ```sh
  npx skills add <owner/repo> -g -y -s <skill名> -a '*'
  ```

  `-a '*'` 装到全部 agent 目录（含全局层 `~/.agents/skills`；个别 agent 不支持全局安装，报一两条失败无害）。`-l` 只列出仓库里的 skill 不安装。
- 安装工具在 `~/.agents/.skill-lock.json` 记录来源与版本——归其自己维护（团队可复现是项目仓库侧的事），yomi 不读不写；在扫描目录之外，天然不干扰。
- 第三方 frontmatter 的 `name:`/`metadata:`/`allowed-tools:` 字段被忽略（不报错）——yomi 按目录路径取名，与 frontmatter 的 `name:` 不一致时以路径为准。

## SKILL.md 格式

```markdown
---
description: 一句话说清什么时候该用这个 skill（触发词都放这里）
---

正文：操作步骤、约定、注意事项。
```

- frontmatter 本体（`---` 包裹）必填；`description` 解析上不强制（缺省为空串），但空 description 的 skill 在索引里没有触发词、等于自我隐身；`disable-model-invocation: true` 可选（不进索引，只能按名/路径手动加载）。
- name 由路径推导，不写进 frontmatter：`<根>/foo/SKILL.md` → `foo`；嵌套 `<根>/suite/child/SKILL.md` → `suite:child`。
- 只索引各层根目录下的第一级（`foo/SKILL.md`）；更深的嵌套不进索引，由父级 SKILL.md 在正文里路由（指引 agent 去读子文件）。
- 伴生文件（脚本、reference、模板）放 skill 目录内，正文里给相对路径。
