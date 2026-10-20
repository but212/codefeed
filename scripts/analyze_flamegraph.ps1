param(
    [string]$File = "flamegraph.svg",
    [int]$Top = 15,
    [string]$Filter = $null
)

$argsList = @("$PSScriptRoot/analyze_flamegraph.py", $File, "--top", $Top)
if ($Filter) {
    $argsList += @("--filter", $Filter)
}

python @argsList
