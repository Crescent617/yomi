# syntax=docker/dockerfile:1
# yomi 运行时镜像。
#
# CI 里直接打包 build-tui 产出的预编译二进制（不在镜像里编译），
# 本地复刻：`cargo build --release --bin yomi`，把产物放到本目录名为 yomi。
#
# 层设计（拉取增量最小化）：
#   1. 工具链层：内容稳定，几乎不随版本变化，layer cache 命中率高；
#   2. yomi 层：单独一层，每次发版只有这层变化；
#   3. 用户态层：几个目录/环境变量，tiny。
#
# 注意：工具集与 flake.nix 的 dockerImage（nix 本地构建路径）保持一致，
# 改动时两边同步。
FROM debian:trixie-slim

# 1. 工具链层
RUN apt-get update && apt-get install -y --no-install-recommends \
      bash coreutils curl ca-certificates diffutils findutils gawk git \
      grep sed tar gzip jq ncurses-term openssh-client procps python3 \
      ripgrep tmux which xz-utils \
    && rm -rf /var/lib/apt/lists/*

# 2. yomi 单独一层
COPY yomi /usr/local/bin/yomi

# 3. 用户态（与 nix 镜像一致：root + /home/yomi，可选工具链由 agent 运行时自装）
RUN mkdir -p /home/yomi
ENV HOME=/home/yomi \
    LANG=C.UTF-8
WORKDIR /home/yomi
ENTRYPOINT ["yomi", "daemon", "start"]
