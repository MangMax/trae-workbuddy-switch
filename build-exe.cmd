@echo off
REM ============================================================================
REM  BuddySwitch 一键打包（双击即可）—— Windows NSIS 安装包 (.exe)
REM
REM  等价于：  powershell -NoProfile -ExecutionPolicy Bypass ^
REM             -File scripts\package-windows.ps1
REM
REM  产物落到 deliverables\ （安装包 + updater 用的 .sig）。
REM  透传参数，例如：build-exe.cmd -Mode debug   /   build-exe.cmd -SkipStamp
REM ============================================================================
setlocal
cd /d "%~dp0"

echo.
echo === BuddySwitch 一键打包 ===
echo.

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\package-windows.ps1" %*
set "RC=%ERRORLEVEL%"

echo.
if not "%RC%"=="0" (
  echo [FAILED] 打包失败（exit %RC%），请查看上方输出与 logs\ 目录下的构建日志。
) else (
  echo [OK] 打包完成，安装包在 deliverables\ 目录。
)

REM 仅在**双击启动**时暂停（被脚本调用时不阻塞）：
REM 双击时 %CMDCMDLINE% 里会包含本脚本名。
echo %CMDCMDLINE% | find /i "%~nx0" >nul
if not errorlevel 1 pause

exit /b %RC%
