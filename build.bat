@echo off
setlocal enabledelayedexpansion
chcp 65001 >nul

echo ============================================
echo   NClientV4 - Build EXE
echo ============================================
echo.

:: ── cargo / rustc to PATH  ──────────────────
set "CARGO_BIN=%USERPROFILE%\.cargo\bin"
if exist "%CARGO_BIN%\cargo.exe" (
    set "PATH=%CARGO_BIN%;%PATH%"
) else (
    echo [ERROR] cargo not found at %CARGO_BIN%
    echo Please install Rust from https://rustup.rs
    exit /b 1
)

:: ── node / npm ────────────────────────────────
where node >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERROR] Node.js not found in PATH
    exit /b 1
)

cd /d "%~dp0"

:: ──Build Fronted ───────────────────────────────
echo [1/2] Building frontend (Vite)...
call npm run build
if %errorlevel% neq 0 (
    echo [ERROR] Frontend build failed
    exit /b 1
)
echo       Done.  Output: dist\
echo.

:: ── 2. Rust ──────────────────────────
echo [2/2] Building Rust backend (release)...
cd src-tauri
cargo build --release
if %errorlevel% neq 0 (
    echo [ERROR] Rust build failed
    cd ..
    exit /b 1
)
cd ..

:: ── output ──────────────────────────────────────
set "EXE=%CD%\src-tauri\target\release\nclientv4.exe"

echo.
echo ============================================
echo   Build successful!
echo.
echo   EXE: %EXE%
echo ============================================

endlocal
