import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "..");
const packageJson = JSON.parse(
  readFileSync(join(repoRoot, "package.json"), "utf8"),
);
const version = packageJson.version;
const tauriCli = join(
  repoRoot,
  "node_modules",
  "@tauri-apps",
  "cli",
  "tauri.js",
);
const requestedTarget = process.env.ANDROID_BUILD_TARGET ?? "armv7";
const requireSigning = process.env.ANDROID_REQUIRE_SIGNING === "1";
const targetConfig = {
  armv7: {
    rustTarget: "armv7-linux-androideabi",
    outputDirectory: "arm",
    artifactAbi: "armeabi-v7a",
    description: "32-bit ARM",
  },
  aarch64: {
    rustTarget: "aarch64-linux-android",
    outputDirectory: "arm64",
    artifactAbi: "arm64-v8a",
    description: "64-bit ARM",
  },
}[requestedTarget];

if (!targetConfig) {
  throw new Error(
    `Unsupported ANDROID_BUILD_TARGET: ${requestedTarget}. Use armv7 or aarch64.`,
  );
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    env: process.env,
    stdio: "inherit",
  });

  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}`);
  }
}

function findAndroidSdk() {
  const candidates = [
    process.env.ANDROID_HOME,
    process.env.ANDROID_SDK_ROOT,
    process.platform === "win32" && process.env.LOCALAPPDATA
      ? join(process.env.LOCALAPPDATA, "Android", "Sdk")
      : undefined,
    process.env.HOME ? join(process.env.HOME, "Android", "Sdk") : undefined,
  ].filter(Boolean);

  return candidates.find((candidate) => existsSync(candidate));
}

function findApkSigner(androidSdk) {
  const buildToolsDir = join(androidSdk, "build-tools");
  if (!existsSync(buildToolsDir)) return undefined;

  const versions = readdirSync(buildToolsDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort((left, right) => right.localeCompare(left, undefined, { numeric: true }));

  for (const buildToolsVersion of versions) {
    const candidate = join(
      buildToolsDir,
      buildToolsVersion,
      "lib",
      "apksigner.jar",
    );
    if (existsSync(candidate)) return candidate;
  }

  return undefined;
}

function findJava() {
  if (process.env.JAVA_HOME) {
    const executable = process.platform === "win32" ? "java.exe" : "java";
    const candidate = join(process.env.JAVA_HOME, "bin", executable);
    if (existsSync(candidate)) return candidate;
  }
  return "java";
}

console.log(
  `Building the Android APK for ${targetConfig.description} (${targetConfig.artifactAbi})...`,
);
run("rustup", ["target", "add", targetConfig.rustTarget]);
if (!existsSync(tauriCli)) {
  throw new Error("Tauri CLI not found. Run npm ci before building.");
}
run(process.execPath, [
  tauriCli,
  "android",
  "build",
  "--apk",
  "--target",
  requestedTarget,
  "--split-per-abi",
  "--ci",
]);

const releaseDir = join(
  repoRoot,
  "src-tauri",
  "gen",
  "android",
  "app",
  "build",
  "outputs",
  "apk",
  targetConfig.outputDirectory,
  "release",
);
const unsignedApk = existsSync(releaseDir)
  ? readdirSync(releaseDir)
      .filter((name) => name.endsWith("-unsigned.apk"))
      .map((name) => join(releaseDir, name))[0]
  : undefined;

if (!unsignedApk) {
  throw new Error(`No unsigned armv7 APK found under ${releaseDir}`);
}

const artifactDir = join(repoRoot, "artifacts");
mkdirSync(artifactDir, { recursive: true });

const keystorePath = process.env.ANDROID_KEYSTORE_PATH
  ? resolve(process.env.ANDROID_KEYSTORE_PATH)
  : undefined;
const androidSdk = findAndroidSdk();
const apkSignerJar = androidSdk ? findApkSigner(androidSdk) : undefined;

if (keystorePath && existsSync(keystorePath) && apkSignerJar) {
  const signedApk = join(
    artifactDir,
    `NClientV4-${version}-android-${targetConfig.artifactAbi}.apk`,
  );
  const keystorePassword = process.env.ANDROID_KEYSTORE_PASSWORD;
  const keyAlias = process.env.ANDROID_KEY_ALIAS;
  const keyPassword = process.env.ANDROID_KEY_PASSWORD;
  if (!keystorePassword || !keyAlias || !keyPassword) {
    throw new Error(
      "ANDROID_KEYSTORE_PASSWORD, ANDROID_KEY_ALIAS, and ANDROID_KEY_PASSWORD are required for signing.",
    );
  }

  const java = findJava();
  run(java, [
    "-jar",
    apkSignerJar,
    "sign",
    "--ks",
    keystorePath,
    "--ks-pass",
    `pass:${keystorePassword}`,
    "--ks-key-alias",
    keyAlias,
    "--key-pass",
    `pass:${keyPassword}`,
    "--out",
    signedApk,
    "--in",
    unsignedApk,
  ]);
  run(java, ["-jar", apkSignerJar, "verify", "--verbose", signedApk]);
  console.log(`Signed Android APK created: ${signedApk}`);
} else {
  if (requireSigning) {
    throw new Error(
      "Signing tools or keystore not found. A signed APK is required for this build.",
    );
  }
  const copiedApk = join(
    artifactDir,
    `NClientV4-${version}-android-${targetConfig.artifactAbi}-unsigned.apk`,
  );
  copyFileSync(unsignedApk, copiedApk);
  console.warn(
    `Signing tools or keystore not found; unsigned APK copied to ${copiedApk}`,
  );
  console.warn("Set ANDROID_KEYSTORE_PATH and Android SDK variables to sign it.");
}
