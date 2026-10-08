@echo off
setlocal
chcp 65001 >nul
cd /d "%~dp0"

echo ============================================
echo   NClientV4 - Signed Android APK Build
echo ============================================
echo.

set "BUILD_TARGET=aarch64"
set "INSTALL_AFTER_BUILD=0"

if /I "%~1"=="--install" (
    set "INSTALL_AFTER_BUILD=1"
) else if not "%~1"=="" (
    set "BUILD_TARGET=%~1"
)
if /I "%~2"=="--install" (
    set "INSTALL_AFTER_BUILD=1"
) else if not "%~2"=="" (
    echo [ERROR] Unsupported option: %~2
    echo Usage: %~nx0 [aarch64^|armv7] [--install]
    exit /b 2
)
if not "%~3"=="" (
    echo [ERROR] Too many arguments.
    echo Usage: %~nx0 [aarch64^|armv7] [--install]
    exit /b 2
)

if /I "%BUILD_TARGET%"=="aarch64" (
    set "ARTIFACT_ABI=arm64-v8a"
) else if /I "%BUILD_TARGET%"=="armv7" (
    set "ARTIFACT_ABI=armeabi-v7a"
) else (
    echo [ERROR] Unsupported target: %BUILD_TARGET%
    echo Usage: %~nx0 [aarch64^|armv7] [--install]
    exit /b 2
)

where node >nul 2>&1
if errorlevel 1 (
    echo [ERROR] Node.js was not found in PATH.
    exit /b 1
)

where rustup >nul 2>&1
if errorlevel 1 (
    echo [ERROR] rustup was not found in PATH.
    exit /b 1
)

if not exist "node_modules\@tauri-apps\cli\tauri.js" (
    echo [ERROR] Project dependencies are missing. Run npm ci first.
    exit /b 1
)

if not defined ANDROID_SDK_ROOT if defined ANDROID_HOME set "ANDROID_SDK_ROOT=%ANDROID_HOME%"
if not defined ANDROID_SDK_ROOT if defined LOCALAPPDATA set "ANDROID_SDK_ROOT=%LOCALAPPDATA%\Android\Sdk"
if not defined ANDROID_SDK_ROOT (
    echo [ERROR] Android SDK was not found. Set ANDROID_SDK_ROOT or ANDROID_HOME.
    exit /b 1
)
if not exist "%ANDROID_SDK_ROOT%\build-tools" (
    echo [ERROR] Android SDK Build Tools were not found under:
    echo         %ANDROID_SDK_ROOT%\build-tools
    exit /b 1
)

if "%INSTALL_AFTER_BUILD%"=="1" (
    if not exist "%ANDROID_SDK_ROOT%\platform-tools\adb.exe" (
        echo [ERROR] adb.exe was not found under the Android SDK.
        exit /b 1
    )
    "%ANDROID_SDK_ROOT%\platform-tools\adb.exe" get-state >nul 2>&1
    if errorlevel 1 (
        echo [ERROR] No single authorized Android device is available.
        echo         Connect one device and accept its USB debugging prompt.
        exit /b 1
    )
)

if not defined ANDROID_KEYSTORE_PATH set "ANDROID_KEYSTORE_PATH=%CD%\src-tauri\nclientv4.keystore"
if not exist "%ANDROID_KEYSTORE_PATH%" (
    echo [ERROR] Signing keystore was not found:
    echo         %ANDROID_KEYSTORE_PATH%
    exit /b 1
)

rem These defaults match the existing project keystore. Override them through
rem environment variables when using a different signing certificate.
if not defined ANDROID_KEYSTORE_PASSWORD set "ANDROID_KEYSTORE_PASSWORD=nclientv4"
if not defined ANDROID_KEY_ALIAS set "ANDROID_KEY_ALIAS=nclientv4"
if not defined ANDROID_KEY_PASSWORD set "ANDROID_KEY_PASSWORD=%ANDROID_KEYSTORE_PASSWORD%"
set "ANDROID_BUILD_TARGET=%BUILD_TARGET%"
set "ANDROID_REQUIRE_SIGNING=1"

for /f "usebackq delims=" %%V in (`node -p "require('./package.json').version"`) do set "APP_VERSION=%%V"
if not defined APP_VERSION (
    echo [ERROR] Unable to read the application version from package.json.
    exit /b 1
)

echo Target:    %BUILD_TARGET%
echo Keystore:  %ANDROID_KEYSTORE_PATH%
echo Output:    artifacts\NClientV4-%APP_VERSION%-android-%ARTIFACT_ABI%.apk
echo.

node scripts\build-android-legacy.mjs
if errorlevel 1 (
    echo.
    echo [ERROR] Android build or APK signing failed.
    exit /b 1
)

set "SIGNED_APK=%CD%\artifacts\NClientV4-%APP_VERSION%-android-%ARTIFACT_ABI%.apk"
if not exist "%SIGNED_APK%" (
    echo.
    echo [ERROR] A signed APK was not produced. Check the Android SDK signing tools.
    exit /b 1
)

if "%INSTALL_AFTER_BUILD%"=="1" (
    echo.
    echo Installing the signed APK on the connected device...
    "%ANDROID_SDK_ROOT%\platform-tools\adb.exe" install -r "%SIGNED_APK%"
    if errorlevel 1 (
        echo [ERROR] APK installation failed.
        exit /b 1
    )
    "%ANDROID_SDK_ROOT%\platform-tools\adb.exe" shell pm path com.nclientv4.app
    if errorlevel 1 (
        echo [ERROR] The installed application package could not be verified.
        exit /b 1
    )
)

echo.
echo ============================================
echo   Signed Android APK created successfully
echo   %SIGNED_APK%
echo ============================================

endlocal
