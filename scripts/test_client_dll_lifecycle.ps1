param(
    [Parameter(Mandatory = $true)]
    [string]$DllPath,

    [ValidateRange(1, 100000)]
    [int]$Iterations = 1000
)

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class NativeLibraryLifecycle
{
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    public static extern IntPtr LoadLibraryW(string fileName);

    [DllImport("kernel32.dll", CharSet = CharSet.Ansi, ExactSpelling = true, SetLastError = true)]
    public static extern IntPtr GetProcAddress(IntPtr module, string procedureName);

    [DllImport("kernel32.dll", ExactSpelling = true, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool FreeLibrary(IntPtr module);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    public static extern IntPtr GetModuleHandleW(string moduleName);
}
"@

$resolvedDllPath = (Resolve-Path -LiteralPath $DllPath).Path
$moduleName = [IO.Path]::GetFileName($resolvedDllPath)

$stream = [IO.File]::OpenRead($resolvedDllPath)
try {
    $reader = [IO.BinaryReader]::new($stream)
    $stream.Position = 0x3c
    $peHeaderOffset = $reader.ReadInt32()
    $stream.Position = $peHeaderOffset + 4
    $machine = $reader.ReadUInt16()
}
finally {
    $stream.Dispose()
}

$expectedMachine = if ([Environment]::Is64BitProcess) { 0x8664 } else { 0x014c }
if ($machine -ne $expectedMachine) {
    $processArchitecture = if ([Environment]::Is64BitProcess) { "x64" } else { "x86" }
    throw "DLL architecture does not match the $processArchitecture PowerShell process"
}

if ([NativeLibraryLifecycle]::GetModuleHandleW($moduleName) -ne [IntPtr]::Zero) {
    throw "$moduleName is already loaded; run this test in a clean PowerShell process"
}

for ($iteration = 1; $iteration -le $Iterations; $iteration++) {
    $module = [NativeLibraryLifecycle]::LoadLibraryW($resolvedDllPath)
    if ($module -eq [IntPtr]::Zero) {
        $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
        throw "LoadLibraryW failed at iteration $iteration with Win32 error $errorCode"
    }

    try {
        $entryPoint = [NativeLibraryLifecycle]::GetProcAddress($module, "DllCanUnloadNow")
        if ($entryPoint -eq [IntPtr]::Zero) {
            $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
            throw "DllCanUnloadNow was not exported at iteration $iteration (Win32 error $errorCode)"
        }
    }
    finally {
        if (-not [NativeLibraryLifecycle]::FreeLibrary($module)) {
            $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
            throw "FreeLibrary failed at iteration $iteration with Win32 error $errorCode"
        }
    }

    if ([NativeLibraryLifecycle]::GetModuleHandleW($moduleName) -ne [IntPtr]::Zero) {
        throw "$moduleName remained loaded after iteration $iteration"
    }
}

Write-Host "PASS: loaded and unloaded $resolvedDllPath $Iterations times"
