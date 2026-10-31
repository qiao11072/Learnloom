. (Join-Path $PSScriptRoot 'env.ps1')
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = if (Test-Path $vswhere) { (& $vswhere -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -format json | ConvertFrom-Json) } else { @() }
$webviews = foreach ($regPath in @('HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients','HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients')) {
  Get-ChildItem $regPath -ErrorAction SilentlyContinue | ForEach-Object {
    $record=Get-ItemProperty $_.PSPath
    if ($record.name -match 'WebView2') { @{name=$record.name;version=$record.pv} }
  }
}
$result = [ordered]@{
  checked_at = (Get-Date).ToString('o')
  os = [Environment]::OSVersion.VersionString
  node = (& node --version)
  npm = (& npm --version)
  rustc = (& rustc --version)
  cargo = (& cargo --version)
  msvc_toolset = $msvc.Name
  windows_sdk = $sdk.Name
  visual_studio = @($vs | Select-Object installationVersion,installationPath,isComplete)
  webview2 = @($webviews)
  scope = 'Project-local Node selection and Rust installation; system MSVC/SDK added to existing Visual Studio'
}
$result | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 (Join-Path $PSScriptRoot '../docs/spikes/artifacts/environment.json')
$result | ConvertTo-Json -Depth 8
