#!/bin/sh
# v0.10.53 升级配套：卸掉过渡用 GUI 登录 LaunchAgent。
# GUI 登录项改为设置页开关（SMAppService 标准机制）后，这个 LaunchAgent
# 必须卸掉——否则用户关掉开关仍会被它拉起来（UI 与系统行为不一致）。
# 幂等：没装、没加载都是直接成功。
LABEL=com.yomi.gui
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
WAS=$([ -f "$PLIST" ] && echo present || echo absent)
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null
rm -f "$PLIST"
echo "LaunchAgent $LABEL unloaded (was $WAS)"
