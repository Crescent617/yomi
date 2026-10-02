#!/bin/sh
# demo init 钩子：装完/刷新后从已装目录执行一次。环境注入
# YOMI_DATA_DIR，PATH 含 <data_dir>/bin——可按名调已挂的 bin 命令。
# 幂等是约定：每次 install/refresh 都会重新执行。
echo "demo extension ready (data_dir=$YOMI_DATA_DIR)"
