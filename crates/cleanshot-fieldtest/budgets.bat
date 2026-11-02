@echo off
rem CHARTER 10: measures every CHARTER 8 budget on this PC and leaves only results.txt here.
setlocal
cd /d "%~dp0"

tasklist /FI "IMAGENAME eq cleanshot.exe" | find /I "cleanshot.exe" >nul
if not errorlevel 1 (
    echo Cleanshot is running. Quit it from its tray icon, then run this file again.
    pause
    exit /b 1
)

echo This measures Cleanshot's performance budgets. It takes about 2 minutes.
echo Do not touch the keyboard or mouse until it says it is done.
echo It opens and closes the capture overlay and an editor window, and it replaces
echo what is on the clipboard.
pause

del /q cleanshot.toml cleanshot.log idle.log p01.log overlays.txt p02.txt probe.md sac.txt results.txt 2>nul

echo Smart App Control state: > sac.txt
reg query "HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy" /v VerifiedAndReputablePolicyState >> sac.txt 2>&1

rem The waits are pings: timeout refuses the redirected input of release.yml's trial run.
echo [0/4] Warm-up start, not measured
rem D-97: the first start of a new exe waits for antivirus to scan it; P-03 is Cleanshot's own start.
type nul > cleanshot.toml
start "" cleanshot.exe --log
call :wait_for "perf P-03 " 120
taskkill /IM cleanshot.exe /F >nul 2>&1
ping -n 2 127.0.0.1 >nul
del /q cleanshot.log 2>nul

echo [1/4] Idle start: P-03, P-04, P-05
start "" cleanshot.exe --log
rem P-04 and P-05 are logged 10 s after the tray is ready, however long the start took.
call :wait_for "perf P-05 " 120
taskkill /IM cleanshot.exe /F >nul 2>&1
ping -n 2 127.0.0.1 >nul
move /y cleanshot.log idle.log >nul 2>&1

echo [2/4] Hotkey to overlay, and memory after use: P-01, P-04
> cleanshot.toml echo [hotkeys]
>> cleanshot.toml echo region = "Ctrl+Shift+4"
start "" cleanshot.exe --log
ping -n 4 127.0.0.1 >nul
cleanshot-fieldtest.exe --overlays cleanshot.log > overlays.txt 2>&1
taskkill /IM cleanshot.exe /F >nul 2>&1
ping -n 2 127.0.0.1 >nul
move /y cleanshot.log p01.log >nul 2>&1

echo [3/4] Copy of a 4K region: P-02
p02.exe p02_ --nocapture > p02.txt 2>&1

echo [4/4] Probe and editor bench: P-07, P-08, monitors and scaling
cleanshot-fieldtest.exe --report probe.md >nul 2>&1

cleanshot-fieldtest.exe --budgets --exe cleanshot.exe --idle idle.log --overlays p01.log --copy p02.txt --bench probe.md > results.txt 2>&1
for %%F in (sac.txt probe.md overlays.txt p02.txt idle.log p01.log) do (
    >> results.txt echo.
    >> results.txt echo ===== %%F
    if exist %%F (type %%F >> results.txt) else (>> results.txt echo missing)
)
del /q cleanshot.toml cleanshot.log idle.log p01.log overlays.txt p02.txt probe.md sac.txt 2>nul

echo.
echo Done. Attach this file to a GitHub issue:
echo %~dp0results.txt
pause
exit /b 0

:wait_for
set waited=0
:wait_for_next
findstr /c:%1 cleanshot.log >nul 2>&1 && exit /b 0
if %waited% geq %2 exit /b 1
set /a waited+=1
ping -n 2 127.0.0.1 >nul
goto wait_for_next
