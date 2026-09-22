@echo off
REM whisperexe yerel sunucu (Yonetim ^> Sunucu ^> "Sunucuyu baslat" bunu acar).
REM Kullanim: broker.exe ile AYNI dizine koyup calistir (istemcideki dugme
REM otomatik bulur: uygulama dizini, kurulum dizini, C:\whisper\bin).
setlocal
if not defined BROKER_ADDR set BROKER_ADDR=127.0.0.1:8899
if "%BROKER_SECRET%"=="" (
  echo BROKER_SECRET bos; once ortama yazin ya da asagidaki satira gomun.
  pause
  exit /b 1
)
if not defined BROKER_LEDGER_PATH set BROKER_LEDGER_PATH=%~dp0data\ledger.jsonl
if not exist "%~dp0data" mkdir "%~dp0data" >nul 2>&1
echo whisperexe sunucu basliyor: %BROKER_ADDR%  defter=%BROKER_LEDGER_PATH%
"%~dp0broker.exe"
