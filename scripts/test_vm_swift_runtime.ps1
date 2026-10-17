param([string]$SourceScript = (Join-Path $PSScriptRoot 'vm_test_client_composition.sh'))
$ErrorActionPreference = 'Stop'
$template = [IO.File]::ReadAllText($SourceScript)
$match = [regex]::Match($template, '(?s)cat > "\$ps1" <<''PS1''\r?\n(.*?)\r?\nPS1')
if (!$match.Success) { throw 'VM test PowerShell template not found' }
$tokens = $null
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseInput($match.Groups[1].Value, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw 'VM test PowerShell template failed to parse' }
$functions = @($ast.FindAll({param($node)
  $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Resolve-LlamaCpuRuntime'
}, $true))
if ($functions.Count -ne 1) { throw 'Expected exactly one runtime resolver' }
. ([ScriptBlock]::Create($functions[0].Extent.Text))

function Assert-True([bool]$Condition, [string]$Message) {
  if (!$Condition) { throw $Message }
}
function Assert-Fails([scriptblock]$Action, [string]$Message) {
  $failed = $false
  try { & $Action | Out-Null } catch { $failed = $true }
  Assert-True $failed $Message
}

$originalTemp = $env:TEMP
$root = Join-Path $originalTemp ('azookey-swift-runtime-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
try {
  $env:TEMP = Join-Path $root 'downloads'
  New-Item -ItemType Directory -Path $env:TEMP | Out-Null
  $payload = Join-Path $root 'payload'
  New-Item -ItemType Directory -Path $payload | Out-Null
  foreach ($name in @('llama.lib', 'llama.dll', 'ggml.dll', 'ggml-base.dll', 'ggml-cpu.dll', 'ggml-rpc.dll', 'llava_shared.dll')) {
    [IO.File]::WriteAllText((Join-Path $payload $name), ('fixture-' + $name))
  }
  $script:fixtureArchive = Join-Path $root 'complete.zip'
  Compress-Archive -Path (Join-Path $payload '*') -DestinationPath $script:fixtureArchive
  $script:requests = 0
  $script:networkFailure = $false
  function Invoke-WebRequest {
    param([string]$Uri, [string]$OutFile)
    $script:requests++
    if ($script:networkFailure) { throw 'Controlled download failure' }
    Assert-True ($Uri.StartsWith('https://') -and $Uri.EndsWith('.zip')) 'Expected a TLS archive download'
    Copy-Item -LiteralPath $script:fixtureArchive -Destination $OutFile
  }

  $cache = Join-Path $root 'empty-cache'
  $source = Join-Path $root 'cold-source'
  $resolved = Resolve-LlamaCpuRuntime -CacheRoot $cache -SourceDir $source 6>$null
  Assert-True ($script:requests -eq 1) 'Cold cache did not fetch a runtime'
  Assert-True ($resolved.StartsWith($source)) 'Cold runtime escaped the owned source tree'
  Assert-True (Test-Path (Join-Path $resolved 'ggml.dll') -PathType Leaf) 'Full archive contents were not retained'
  Assert-True (!(Test-Path $cache)) 'Optional external cache was unexpectedly populated'

  $cached = Join-Path $cache 'llama_cpu'
  New-Item -ItemType Directory -Path $cached -Force | Out-Null
  Copy-Item -Path (Join-Path $payload '*') -Destination $cached
  $resolved = Resolve-LlamaCpuRuntime -CacheRoot $cache -SourceDir (Join-Path $root 'cache-hit-source') 6>$null
  Assert-True ($resolved -eq $cached -and $script:requests -eq 1) 'Complete cache was not reused'

  $incomplete = Join-Path $root 'incomplete-cache'
  New-Item -ItemType Directory -Path $incomplete | Out-Null
  $incompleteRuntime = Join-Path $incomplete 'llama_cpu'
  New-Item -ItemType Directory -Path $incompleteRuntime | Out-Null
  Get-ChildItem -Path $payload -File | Where-Object Name -ne 'ggml.dll' | Copy-Item -Destination $incompleteRuntime
  $sentinel = Join-Path $incomplete 'keep.txt'
  [IO.File]::WriteAllText($sentinel, 'unrelated fixture')
  $resolved = Resolve-LlamaCpuRuntime -CacheRoot $incomplete -SourceDir (Join-Path $root 'fallback-source') 6>$null
  Assert-True ($script:requests -eq 2 -and (Test-Path $sentinel)) 'Fallback changed unrelated cache data'
  Assert-True (!(Test-Path (Join-Path $incompleteRuntime 'ggml.dll'))) 'Partial external cache was modified'

  $sourceHit = Join-Path $root 'source-runtime-hit'
  $sourceRuntime = Join-Path $sourceHit 'llama_cpu'
  New-Item -ItemType Directory -Path $sourceRuntime -Force | Out-Null
  Copy-Item -Path (Join-Path $payload '*') -Destination $sourceRuntime
  $resolved = Resolve-LlamaCpuRuntime -CacheRoot $incomplete -SourceDir $sourceHit 6>$null
  Assert-True ($resolved -eq $sourceRuntime -and $script:requests -eq 2) 'Complete source runtime was not reused'

  $script:networkFailure = $true
  $failureSource = Join-Path $root 'network-failure-source'
  Assert-Fails { Resolve-LlamaCpuRuntime -CacheRoot $incomplete -SourceDir $failureSource 6>$null } 'Download failure was swallowed'
  Assert-True (@(Get-ChildItem -Path $failureSource -Filter '.llama_cpu-*').Count -eq 0) 'Failed download left a published runtime'
  $script:networkFailure = $false

  $brokenPayload = Join-Path $root 'broken-payload'
  New-Item -ItemType Directory -Path $brokenPayload | Out-Null
  Get-ChildItem -Path $payload -File | Where-Object Name -ne 'ggml.dll' | Copy-Item -Destination $brokenPayload
  $script:fixtureArchive = Join-Path $root 'broken.zip'
  Compress-Archive -Path (Join-Path $brokenPayload '*') -DestinationPath $script:fixtureArchive
  $brokenSource = Join-Path $root 'broken-source'
  Assert-Fails { Resolve-LlamaCpuRuntime -CacheRoot $incomplete -SourceDir $brokenSource 6>$null } 'Incomplete archive was accepted'
  Assert-True (@(Get-ChildItem -Path $brokenSource -Filter '.llama_cpu-*').Count -eq 0) 'Incomplete extraction was retained'
  Assert-True (@(Get-ChildItem -Path $env:TEMP).Count -eq 0) 'Owned download archives were not cleaned up'

  @{task_status='succeeded'; cases=6; downloads_mocked=$true; network_requests=0} | ConvertTo-Json -Compress
} finally {
  $env:TEMP = $originalTemp
  Remove-Item -LiteralPath $root -Recurse -Force
}
