# Full-screen black window that flashes white and beeps at the same instant every 2 s.
# Used to measure audio/video sync of a recording. Run with: pwsh -STA -File av-flash.ps1
param([int]$Seconds = 14, [string]$Ready = "")
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$rate = 48000; $n = [int]($rate * 0.25)
$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms)
$bw.Write([Text.Encoding]::ASCII.GetBytes("RIFF")); $bw.Write([int](36 + $n * 2))
$bw.Write([Text.Encoding]::ASCII.GetBytes("WAVEfmt ")); $bw.Write([int]16); $bw.Write([int16]1); $bw.Write([int16]1)
$bw.Write([int]$rate); $bw.Write([int]($rate * 2)); $bw.Write([int16]2); $bw.Write([int16]16)
$bw.Write([Text.Encoding]::ASCII.GetBytes("data")); $bw.Write([int]($n * 2))
for ($i = 0; $i -lt $n; $i++) { $bw.Write([int16](12000 * [math]::Sin(2 * [math]::PI * 1000 * $i / $rate))) }
$bw.Flush(); $ms.Position = 0
$player = New-Object System.Media.SoundPlayer($ms)
$player.Load()
$f = New-Object System.Windows.Forms.Form
$f.FormBorderStyle = "None"; $f.WindowState = "Maximized"; $f.TopMost = $true; $f.BackColor = [System.Drawing.Color]::Black
$t = New-Object System.Windows.Forms.Timer
$t.Interval = 2000
$off = New-Object System.Windows.Forms.Timer
$off.Interval = 250
$off.Add_Tick({ $f.BackColor = [System.Drawing.Color]::Black; $f.Refresh(); $off.Stop() })
$t.Add_Tick({ $player.Play(); $f.BackColor = [System.Drawing.Color]::White; $f.Refresh(); $off.Start() })
$end = New-Object System.Windows.Forms.Timer
$end.Interval = $Seconds * 1000
$end.Add_Tick({ $f.Close() })
# Tell the test the window is up (it starts recording only then).
$f.Add_Shown({ $t.Start(); $end.Start(); if ($Ready) { "shown" | Out-File $Ready } })
[System.Windows.Forms.Application]::Run($f)
