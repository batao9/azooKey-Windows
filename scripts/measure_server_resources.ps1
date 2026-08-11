<#
.SYNOPSIS
Samples azookey-server CPU, memory, and priority without instrumenting the input path.

.EXAMPLE
powershell -NoProfile -File scripts/measure_server_resources.ps1 `
    -DurationSeconds 180 > server-resources.tsv
#>

param(
    [ValidateRange(1, 86400)]
    [int]$DurationSeconds = 95,

    [ValidateRange(100, 60000)]
    [int]$SampleIntervalMilliseconds = 1000,

    [ValidateNotNullOrEmpty()]
    [string]$ProcessName = "azookey-server"
)

$ErrorActionPreference = "Stop"
$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

"timestamp_utc`telapsed_ms`tpid`tcpu_seconds`tprivate_bytes`tworking_set_bytes`tpriority_class`tthread_count`thandle_count"

while ($stopwatch.Elapsed.TotalSeconds -lt $DurationSeconds) {
    $process = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue |
        Sort-Object StartTime -Descending |
        Select-Object -First 1

    if ($null -ne $process) {
        try {
            $process.Refresh()
            @(
                [DateTime]::UtcNow.ToString("O")
                [int64]$stopwatch.ElapsedMilliseconds
                $process.Id
                $process.TotalProcessorTime.TotalSeconds.ToString(
                    "F6",
                    [Globalization.CultureInfo]::InvariantCulture
                )
                $process.PrivateMemorySize64
                $process.WorkingSet64
                $process.PriorityClass
                $process.Threads.Count
                $process.HandleCount
            ) -join "`t"
        } catch [System.InvalidOperationException] {
            # The process exited between lookup and sampling. The next sample
            # will pick up a restarted server without terminating the probe.
        }
    }

    Start-Sleep -Milliseconds $SampleIntervalMilliseconds
}
