
@echo off
setlocal

set "key=HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced"

for /f "tokens=3" %%A in ('reg query "%key%" /v TaskbarDa 2^>nul') do set "state=%%A"

if /i "%state%"=="0x1" (
    reg add "%key%" /v TaskbarDa /t REG_DWORD /d 0 /f
) else (
    reg add "%key%" /v TaskbarDa /t REG_DWORD /d 1 /f
)

taskkill /f /im explorer.exe >nul 2>&1
start explorer.exe
