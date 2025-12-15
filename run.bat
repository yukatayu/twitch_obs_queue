@echo off
cd /d %~dp0
set CONFIG=config.toml
twitch_obs_queue.exe
pause
