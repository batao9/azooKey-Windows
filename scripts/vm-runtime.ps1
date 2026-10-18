# Shared by build and Swift-test preparation. No work runs while dot-sourcing.
function Resolve-LlamaCpuRuntime {
  param([string]$CacheRoot, [string]$SourceDir)
  $ErrorActionPreference = 'Stop'

  # DLL/linker files in the pinned b4846 CPU archive. Keep the list in sync with
  # the asset: a lib/DLL pair can hide a partially extracted backend.
  $requiredFiles = @("llama.lib", "llama.dll", "ggml.dll", "ggml-base.dll", "ggml-cpu.dll", "ggml-rpc.dll", "llava_shared.dll")
  foreach ($candidate in @((Join-Path $CacheRoot "llama_cpu"), (Join-Path $SourceDir "llama_cpu"))) {
    $missing = @($requiredFiles | Where-Object { !(Test-Path (Join-Path $candidate $_) -PathType Leaf) })
    if ($missing.Count -eq 0) { return $candidate }
  }

  # Download into this run's source tree, never an external clone/cache.
  New-Item -Path $SourceDir -ItemType Directory -Force | Out-Null
  $token = [guid]::NewGuid().ToString('N')
  $archive = Join-Path $env:TEMP ("azookey-llama-" + $token + ".zip")
  $runtime = Join-Path $SourceDir (".llama_cpu-" + $token)
  $complete = $false
  try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri "https://github.com/fkunn1326/llama.cpp/releases/download/b4846/llama-b4846-bin-win-avx-x64.zip" -OutFile $archive | Out-Null
    Expand-Archive -Path $archive -DestinationPath $runtime -Force
    foreach ($file in $requiredFiles) {
      if (!(Test-Path (Join-Path $runtime $file) -PathType Leaf)) {
        throw "Downloaded llama CPU runtime is missing $file"
      }
    }
    $complete = $true
    return $runtime
  } finally {
    if (Test-Path $archive) { Remove-Item -LiteralPath $archive -Force }
    if (!$complete -and (Test-Path $runtime)) { Remove-Item -LiteralPath $runtime -Recurse -Force }
  }
}

function Copy-LlamaCpuRuntimeForBuild {
  param([string]$CacheRoot, [string]$SourceDir)
  $ErrorActionPreference = 'Stop'
  $runtime = Resolve-LlamaCpuRuntime -CacheRoot $CacheRoot -SourceDir $SourceDir
  $destination = Join-Path $SourceDir 'llama_cpu'
  if ($runtime -ne $destination) {
    New-Item -Path $destination -ItemType Directory -Force | Out-Null
    Copy-Item -Path (Join-Path $runtime '*') -Destination $destination -Recurse -Force
  }
  return $destination
}
