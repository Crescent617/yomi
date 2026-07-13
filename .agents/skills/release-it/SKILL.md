---
name: release-it
description: "Release a new version of yomi. Check current version against tags, bump if needed, tag, and push."
---

# Skill: release-it

发布 yomi 新版本。核心流程：检查当前版本是否已有 tag → 若无则直接发版，若有则先 bump 版本 → 打 tag → push。

## 步骤

### 1. 确认工作区干净

```bash
git diff --quiet && git diff --staged --quiet
```

若不干净，先 `git stash` 或提交当前改动，确保发版 commit 独立清晰。

### 2. 读取当前版本

```bash
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
echo "Current version: $VERSION"
```

### 3. 检查 tag 是否存在

```bash
if git rev-parse "v${VERSION}" >/dev/null 2>&1; then
    echo "Tag v${VERSION} already exists. Need to bump."
    NEED_BUMP=1
else
    echo "Tag v${VERSION} does not exist. Ready to release."
    NEED_BUMP=0
fi
```

### 4. 若需要 bump 版本

使用 `scripts/bump-version.py`。默认 bump `patch`；如需 `minor` 或 `major`，在命令中指定 `--level`。

```bash
# 默认 patch
python scripts/bump-version.py --level patch

# 或 minor / major
python scripts/bump-version.py --level minor
```

脚本会自动同步更新以下文件：
- `Cargo.toml`
- `crates/gui/tauri.conf.json`
- `crates/gui/package.json`
- `crates/gui/package-lock.json`
- `crates/gui/frontend/package.json`
- `crates/gui/frontend/package-lock.json`

### 5. 打 tag

用最终版本（bump 后的或原本的）打 tag：

```bash
RELEASE_VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
git tag -a "v${RELEASE_VERSION}" -m "Release v${RELEASE_VERSION}"
```

### 6. Push

```bash
git push origin main
git push origin "v${RELEASE_VERSION}"
```

（若当前分支不是 `main`，请替换为实际分支名。）

## 注意事项

- `scripts/bump-version.py` 只修改文件内容，**不会自动 git commit**，发版前务必手动 add & commit。
- 若需要设置精确版本而非自动 bump，可用 `--set`：
  ```bash
  python scripts/bump-version.py --set 0.6.0
  ```
- 打 tag 前请确保 CHANGELOG.md（如有）已更新对应版本内容。
