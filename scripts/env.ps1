$taskRoot = Split-Path $PSScriptRoot -Parent
$env:CARGO_HOME = Join-Path $taskRoot '.tools\cargo'
$env:RUSTUP_HOME = Join-Path $taskRoot '.tools\rustup'
$env:npm_config_cache = Join-Path $taskRoot '.tools\npm-cache'
$m0Temp = Join-Path $taskRoot '.tools\tmp'
New-Item -ItemType Directory -Force $m0Temp | Out-Null
$env:TEMP = $m0Temp
$env:TMP = $m0Temp
$nvmCommand = Get-Command nvm -ErrorAction SilentlyContinue
$node22Path = if ($nvmCommand) { Join-Path (Split-Path $nvmCommand.Source -Parent) 'v22.22.1' } else { Join-Path $env:USERPROFILE 'AppData\Local\nvm\v22.22.1' }
if (Test-Path (Join-Path $node22Path 'node.exe')) { $env:PATH = "$node22Path;$env:PATH" }
$env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
$vsRoot = 'C:\Program Files\Microsoft Visual Studio\2022\Community'
$msvc = Get-ChildItem (Join-Path $vsRoot 'VC\Tools\MSVC') -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
$sdkRoot = 'C:\Program Files (x86)\Windows Kits\10'
$sdk = Get-ChildItem (Join-Path $sdkRoot 'Lib') -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
if ($msvc -and $sdk) {
  $env:PATH = "$($msvc.FullName)\bin\Hostx64\x64;$sdkRoot\bin\$($sdk.Name)\x64;$env:PATH"
  $env:INCLUDE = "$($msvc.FullName)\include;$sdkRoot\Include\$($sdk.Name)\ucrt;$sdkRoot\Include\$($sdk.Name)\shared;$sdkRoot\Include\$($sdk.Name)\um;$sdkRoot\Include\$($sdk.Name)\winrt"
  $env:LIB = "$($msvc.FullName)\lib\x64;$sdkRoot\Lib\$($sdk.Name)\ucrt\x64;$sdkRoot\Lib\$($sdk.Name)\um\x64"
  $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = "$($msvc.FullName)\bin\Hostx64\x64\link.exe"
}
