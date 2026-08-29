# syntax=docker/dockerfile:1
# yomi 运行时镜像：CI 打包预编译二进制，不在镜像里编译。
# 本地复刻：`cargo build --release --bin yomi`，把产物放到本目录名为 yomi。
FROM debian:trixie-slim

# 工具链层：内容稳定，layer cache 命中率高
RUN apt-get update && apt-get install -y --no-install-recommends \
      bash coreutils curl ca-certificates diffutils findutils gawk git \
      grep sed tar gzip jq ncurses-term openssh-client procps python3 \
      ripgrep tmux which xz-utils \
    && rm -rf /var/lib/apt/lists/*

# yomi 单独一层，每次发版只有这层变化
COPY yomi /usr/local/bin/yomi

# 用户态：root + /home/yomi，可选工具链由 agent 运行时自装
RUN mkdir -p /home/yomi
ENV HOME=/home/yomi \
    LANG=C.UTF-8
WORKDIR /home/yomi
ENTRYPOINT ["yomi", "daemon", "start"]
