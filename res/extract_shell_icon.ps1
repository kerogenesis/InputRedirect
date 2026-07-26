<#
.SYNOPSIS
    Extracts one icon from the Windows shell library into an .ico file.
#>
[CmdletBinding()]
param(
    [int]$Index = 120,
    [Parameter(Mandatory = $true)][string]$Output,
    [string]$Source = "$env:SystemRoot\System32\shell32.dll"
)

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Drawing

Add-Type -Namespace Shell -Name Icons -MemberDefinition @'
[DllImport("shell32.dll", CharSet = CharSet.Unicode)]
public static extern int ExtractIconEx(string file, int index, IntPtr[] large, IntPtr[] small, int count);

[DllImport("user32.dll")]
public static extern bool DestroyIcon(IntPtr handle);
'@

$large = New-Object IntPtr[] 1
$small = New-Object IntPtr[] 1

if ([Shell.Icons]::ExtractIconEx($Source, $Index, $large, $small, 1) -eq 0 -or $large[0] -eq [IntPtr]::Zero) {
    throw "No icon at index $Index in $Source"
}

try {
    $icon = [System.Drawing.Icon]::FromHandle($large[0])
    $stream = [System.IO.File]::Create($Output)
    try { $icon.Save($stream) } finally { $stream.Dispose() }
}
finally {
    [void][Shell.Icons]::DestroyIcon($large[0])
    if ($small[0] -ne [IntPtr]::Zero) { [void][Shell.Icons]::DestroyIcon($small[0]) }
}

Write-Host "Icon $Index written to $Output"
