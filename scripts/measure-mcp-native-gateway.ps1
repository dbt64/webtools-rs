param(
    [Parameter(Mandatory = $true)][string]$Delivery,
    [Parameter(Mandatory = $true)][string]$BaselineDelivery,
    [Parameter(Mandatory = $true)][string]$PluginPackage,
    [ValidateRange(3,60)][int]$WindowSeconds = 5
)
$ErrorActionPreference = 'Stop'
$scriptFile = Join-Path $PSScriptRoot 'measure-mcp-native-gateway.mjs'
& node --experimental-transform-types $scriptFile --delivery $Delivery --baseline-delivery $BaselineDelivery --plugin-package $PluginPackage --window-seconds $WindowSeconds
exit $LASTEXITCODE
