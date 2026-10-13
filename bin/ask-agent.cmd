@setlocal
@chcp 65001 >nul
@if not defined JWIC_NODE (set "JWIC_NODE=node") else (set ELECTRON_RUN_AS_NODE=1)
@"%JWIC_NODE%" "%~dp0ask-agent.js" %*
