<#
.SYNOPSIS
  Samples memory and CPU of a running Glowframe process the way Task Manager
  shows it ("Working Set - Private") plus CPU load in % of all cores.
.PARAMETER Id
  Process id.
.PARAMETER Seconds
  Sampling duration for the CPU average.
#>
param([Parameter(Mandatory)][int]$Id, [int]$Seconds = 5)

$p = Get-Process -Id $Id
$name = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$Id").Name
$c0 = $p.TotalProcessorTime; $t0 = Get-Date
Start-Sleep -Seconds $Seconds
$p.Refresh(); $c1 = $p.TotalProcessorTime; $t1 = Get-Date
$perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "Name='$name'"
$cores = [Environment]::ProcessorCount
$cpu = ($c1 - $c0).TotalSeconds / ($t1 - $t0).TotalSeconds * 100
[pscustomobject]@{
    PrivateWorkingSetMB = [math]::Round($perf.WorkingSetPrivate / 1MB, 1)
    WorkingSetMB        = [math]::Round($p.WorkingSet64 / 1MB, 1)
    PrivateBytesMB      = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
    CpuPercentTotal     = [math]::Round($cpu / $cores, 2)
    CpuPercentOneCore   = [math]::Round($cpu, 2)
    Threads             = $p.Threads.Count
}
