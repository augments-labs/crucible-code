$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Availability {
    [UnmanagedFunctionPointer(CallingConvention.Winapi, SetLastError=true)]
    public delegate int QuerySandbox(out ulong capabilities);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern IntPtr LoadLibraryExW(string name, IntPtr file, uint flags);
    [DllImport("kernel32.dll", CharSet=CharSet.Ansi, ExactSpelling=true, SetLastError=true)]
    public static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool FreeLibrary(IntPtr module);
}
'@
Write-Output ('OS version=' + [Environment]::OSVersion.Version.ToString() + ' architecture=' + [Runtime.InteropServices.RuntimeInformation]::OSArchitecture)
$probeModule = [Availability]::LoadLibraryExW('processmodel.dll', [IntPtr]::Zero, 0x800)
if ($probeModule -eq [IntPtr]::Zero) {
    Write-Output ('API module_present=false error=' + [Runtime.InteropServices.Marshal]::GetLastWin32Error())
} else {
    try {
        foreach ($probeExport in @('Experimental_CreateProcessInSandbox', 'Experimental_CreateProcessAsUserInSandbox', 'Experimental_QuerySandboxSupport', 'CreateProcessSecurityEnvironment', 'QueryProcessSecurityEnvironmentSupport', 'CloseProcessSecurityEnvironment')) {
            $probeAddress = [Availability]::GetProcAddress($probeModule, $probeExport)
            Write-Output ('API name=' + $probeExport + ' present=' + ($probeAddress -ne [IntPtr]::Zero))
        }
        $probeQueryAddress = [Availability]::GetProcAddress($probeModule, 'Experimental_QuerySandboxSupport')
        if ($probeQueryAddress -ne [IntPtr]::Zero) {
            $probeQuery = [Runtime.InteropServices.Marshal]::GetDelegateForFunctionPointer($probeQueryAddress, [Type][Availability+QuerySandbox])
            [UInt64]$probeCapabilities = 0
            $probeSucceeded = $probeQuery.Invoke([ref]$probeCapabilities)
            $probeQueryError = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
            Write-Output ('CAPABILITY query_return=' + $probeSucceeded + ' bits=' + $probeCapabilities + ' error_if_failed=' + $probeQueryError)
        } else {
            Write-Output 'CAPABILITY query_absent=true availability_unproven=true'
        }
    } finally {
        if (-not [Availability]::FreeLibrary($probeModule)) { throw 'FreeLibrary failed' }
    }
}
$probeSystem = [Environment]::GetFolderPath([Environment+SpecialFolder]::System)
Write-Output ('WSB executable_present=' + (Test-Path -LiteralPath (Join-Path $probeSystem 'wsb.exe') -PathType Leaf))
foreach ($probeFeature in @('Microsoft-Hyper-V-All', 'Containers-DisposableClientVM', 'Containers')) {
    try {
        $probeStates = @(Get-WindowsOptionalFeature -Online -FeatureName $probeFeature -ErrorAction Stop)
        if ($probeStates.Count -ne 1 -or [String]::IsNullOrWhiteSpace([String]$probeStates[0].State)) { throw 'ambiguous or missing optional-feature result' }
        Write-Output ('FEATURE name=' + $probeFeature + ' state=' + $probeStates[0].State)
    } catch {
        Write-Output ('FEATURE name=' + $probeFeature + ' query_unavailable=true')
    }
}
try {
    $probeMachine = Get-CimInstance Win32_ComputerSystem -ErrorAction Stop
    Write-Output ('VM hypervisor_present=' + $probeMachine.HypervisorPresent)
    $probeProcessors = @(Get-CimInstance Win32_Processor -ErrorAction Stop)
    if ($probeProcessors.Count -gt 16) { throw 'processor query bound exceeded' }
    foreach ($probeProcessor in $probeProcessors) {
        Write-Output ('CPU virtualization_firmware=' + $probeProcessor.VirtualizationFirmwareEnabled + ' second_level_translation=' + $probeProcessor.SecondLevelAddressTranslationExtensions)
    }
} catch {
    Write-Output 'VM capability_query_unavailable=true'
}
Write-Output 'COMPLETE readonly=true state_changed=false guest_started=false sandbox_validated=false'
