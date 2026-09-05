# Read-only inventory. Omits command lines, prompts, environment, and credentials.
$ErrorActionPreference = 'Stop'
$snapshot = @(Get-CimInstance Win32_Process)
$index = @{}
foreach ($process in $snapshot) { $index[[int]$process.ProcessId] = $process }
$rows = @(foreach ($process in $snapshot | Where-Object { $_.Name -match '^(claude|codex|codex-code-mode-host)\.exe$' }) {
    $chain = @()
    $current = $process
    $owner = $null
    for ($depth = 0; $depth -lt 16 -and $current; $depth++) {
        $chain += "$($current.Name):$($current.ProcessId)"
        if ($current.Name -match '^laplus(-server)?\.exe$') { $owner = $current.ProcessId; break }
        $parent = $index[[int]$current.ParentProcessId]
        if (!$parent) { $chain += 'parent-missing'; break }
        if ($parent.CreationDate -gt $current.CreationDate) { $chain += 'parent-PID-reused'; break }
        $current = $parent
    }
    [pscustomobject]@{
        PID = $process.ProcessId
        Name = $process.Name
        Started = $process.CreationDate.ToString('s')
        AgeHours = [math]::Round(((Get-Date) - $process.CreationDate).TotalHours, 1)
        WorkingSetMiB = [math]::Round([double]$process.WorkingSetSize / 1MB, 1)
        LaplusPID = $owner
        Ancestry = $chain -join ' <- '
    }
})
$rows | ConvertTo-Json -Depth 3
