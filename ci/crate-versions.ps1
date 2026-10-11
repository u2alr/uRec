# Prints the newest published versions (and MSRV) of candidate dependencies.
foreach ($c in "windows","windows-core","eframe","egui","egui_extras","tray-icon","image","serde","serde_json","winres","embed-resource","raw-window-handle","winit","wgpu","log","simplelog","notify-rust","rfd","directories","dirs","open","arboard","crossbeam-channel","parking_lot","once_cell","thiserror","anyhow") {
  $r = Invoke-RestMethod -Headers @{ "User-Agent" = "urec-ci (github.com/u2alr/uRec)" } "https://crates.io/api/v1/crates/$c" -ErrorAction SilentlyContinue
  if ($r) {
    $v = $r.versions | Where-Object { -not $_.yanked -and $_.num -notmatch '-' } | Select-Object -First 1
    "{0,-18} {1,-10} {2:yyyy-MM-dd} msrv={3} license={4}" -f $c, $v.num, [datetime]$v.created_at, $v.rust_version, $v.license
  } else { "$c ?" }
  Start-Sleep -Milliseconds 300
}
