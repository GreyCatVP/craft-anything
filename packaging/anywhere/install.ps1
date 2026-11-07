# Craft Anything "anywhere" bootstrap (Windows).
# Downloads the official release CLI (original, unmodified), connects it to detected
# agent hosts, enables Deep mode + local Ghidra, and copies the skill to all host dirs.
# Does not edit any upstream source.
param([string]$Version = "0.5.0")
$ErrorActionPreference = "Stop"

function Find-Ghidra {
  $roots = @("$HOME\Tools", "$HOME\Downloads", "$HOME\Desktop", "$HOME", "C:\Program Files", "C:\")
  foreach ($r in $roots) {
    if (-not (Test-Path $r)) { continue }
    $hit = Get-ChildItem -Path $r -Directory -Filter "ghidra_*" -ErrorAction SilentlyContinue |
      Where-Object { Test-Path (Join-Path $_.FullName "support\analyzeHeadless.bat") } |
      Select-Object -First 1
    if ($hit) { return $hit.FullName }
  }
  return $null
}
function Find-Java {
  if ($env:JAVA_HOME) { return $env:JAVA_HOME }
  $hit = Get-ChildItem -Path "$HOME\Tools" -Directory -Filter "jdk*" -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($hit) { return $hit.FullName }
  return $null
}

# 1. Locate or download the CLI.
$cli = (Get-Command craft-anything-cli -ErrorAction SilentlyContinue).Source
if (-not $cli) {
  foreach ($p in @("$HOME\Tools\craft-anything\craft-anything-cli.exe", "$HOME\.local\bin\craft-anything-cli.exe")) {
    if (Test-Path $p) { $cli = $p; break }
  }
}
if (-not $cli) {
  $arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "arm64" } else { "x64" }
  $url = "https://github.com/storytold/craft-anything/releases/download/v$Version/craft-anything-$Version-windows-$arch-portable.zip"
  $tmp = Join-Path $env:TEMP "ca-$Version.zip"
  $dest = "$HOME\Tools\craft-anything"
  Write-Host "Downloading $url"
  Invoke-WebRequest -Uri $url -OutFile $tmp
  New-Item -ItemType Directory -Force -Path $dest | Out-Null
  $extract = Join-Path $env:TEMP "ca-$Version"
  Expand-Archive -Path $tmp -DestinationPath $extract -Force
  $exe = Get-ChildItem -Path $extract -Recurse -Filter "craft-anything-cli.exe" | Select-Object -First 1
  Copy-Item $exe.FullName -Destination $dest -Force
  $cli = Join-Path $dest "craft-anything-cli.exe"
}
Write-Host "CLI: $cli"

# 2. Connect to detected hosts + install bundled skill.
& $cli setup --all-detected --yes

# 3. Deep mode + local Ghidra/Java.
& $cli settings_set --key deep_mode --value true | Out-Null
$ghidra = Find-Ghidra
if ($ghidra) {
  & $cli settings_set --key engines.ghidra.install_dir --value ("`"" + $ghidra + "`"") | Out-Null
  Write-Host "Ghidra: $ghidra"
}
$java = Find-Java
if ($java) { & $cli settings_set --key engines.ghidra.java_home --value ("`"" + $java + "`"") | Out-Null }

# 4. Copy the bundled skill into every known host skill dir (ready even if host not installed).
$src = $null
foreach ($p in @("$HOME\.agents\skills\craft-anything", "$HOME\.qoder\skills\craft-anything")) {
  if (Test-Path $p) { $src = $p; break }
}
if ($src) {
  foreach ($hostDir in @(".qoder", ".claude", ".codex", ".cursor", ".agents")) {
    $dst = Join-Path $HOME "$hostDir\skills\craft-anything"
    New-Item -ItemType Directory -Force -Path (Split-Path $dst) | Out-Null
    Copy-Item -Path $src -Destination $dst -Recurse -Force
  }
  Write-Host "Skill copied to host dirs."
}

# 5. Summary.
& $cli doctor
Write-Host "Done. Restart/reconnect your AI host so its MCP tool list refreshes."