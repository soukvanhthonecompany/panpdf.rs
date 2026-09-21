# PanPDF's side of printing on Windows. pdf_print::windows runs this; it is
# not meant to be run by hand.
#
# What to do comes in PANPDF_* environment variables, and so does every name.
# An environment variable is never parsed as script, so no printer name or
# title can become code. To print, the sheets come on standard input, already
# drawn at the printer's resolution. The answer goes to standard output as
# tab-separated lines: a word, then numbers, or text as UTF-8 in base64 so no
# name can break a line. Sizes are in hundredths of an inch, as
# System.Drawing.Printing states them.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

function Say([string]$word, [object[]]$fields) {
    [Console]::Out.WriteLine((@($word) + $fields) -join "`t")
}

function Text([string]$text) {
    [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($text))
}

# Within four hundredths of an inch: a millimetre, the tolerance papers are
# matched with everywhere else in PanPDF.
function Same([double]$one, [double]$other) {
    [Math]::Abs($one - $other) -le 4
}

function Whole([double]$number) {
    [int][Math]::Round($number)
}

function Settings-For([string]$name) {
    $settings = New-Object System.Drawing.Printing.PrinterSettings
    $settings.PrinterName = $name
    if (-not $settings.IsValid) {
        throw "There is no printer called $name."
    }
    $settings
}

function Sizes([string]$listed) {
    $sizes = @()
    foreach ($pair in ($listed -split ';')) {
        if ($pair) {
            $sizes += , @($pair -split 'x' | ForEach-Object { [double]$_ })
        }
    }
    , $sizes
}

function List {
    foreach ($name in [System.Drawing.Printing.PrinterSettings]::InstalledPrinters) {
        Say 'printer' @(Text $name)
    }
    $default = New-Object System.Drawing.Printing.PrinterSettings
    if ($default.IsValid -and $default.IsDefaultPrinter) {
        Say 'default' @(Text $default.PrinterName)
    }
}

function Ask {
    $settings = Settings-For $env:PANPDF_PRINTER
    $page = $settings.DefaultPageSettings
    Say 'colour' @([int]$settings.SupportsColor)
    Say 'two-sided' @([int]$settings.CanDuplex)
    $resolution = $page.PrinterResolution
    if ($resolution -and $resolution.X -gt 0) {
        Say 'resolution' @($resolution.X)
    }
    Say 'default-paper' @($page.PaperSize.Width, $page.PaperSize.Height)
    $wanted = Sizes $env:PANPDF_SIZES
    foreach ($paper in $settings.PaperSizes) {
        if ($paper.Width -le 0 -or $paper.Height -le 0) {
            continue
        }
        Say 'paper' @($paper.Width, $paper.Height)
        $known = $false
        foreach ($size in $wanted) {
            if ((Same $paper.Width $size[0]) -and (Same $paper.Height $size[1])) {
                $known = $true
            }
        }
        # Asking a driver for a paper's printable area is asking it to make
        # a device context; only the papers PanPDF offers are asked about.
        if (-not $known) {
            continue
        }
        $one = New-Object System.Drawing.Printing.PageSettings($settings)
        $one.PaperSize = $paper
        $one.Landscape = $false
        $area = $one.PrintableArea
        Say 'margin' @(
            $paper.Width, $paper.Height,
            (Whole $area.X), (Whole $area.Y),
            (Whole ($paper.Width - $area.Right)), (Whole ($paper.Height - $area.Bottom))
        )
    }
}

function Print-Sheets {
    $settings = Settings-For $env:PANPDF_PRINTER
    $settings.Copies = [int16]$env:PANPDF_COPIES
    $settings.Collate = $true
    $sides = [int]$env:PANPDF_SIDES
    if ($sides -ne 1) {
        if (-not $settings.CanDuplex) {
            throw 'This printer does not print on both sides of the paper.'
        }
        $settings.Duplex = [System.Drawing.Printing.Duplex]$sides
    } else {
        $settings.Duplex = [System.Drawing.Printing.Duplex]::Simplex
    }
    $want = @($env:PANPDF_PAPER -split 'x' | ForEach-Object { [double]$_ })
    $paper = $null
    foreach ($candidate in $settings.PaperSizes) {
        if ((Same $candidate.Width $want[0]) -and (Same $candidate.Height $want[1])) {
            $paper = $candidate
            break
        }
    }
    if (-not $paper) {
        throw 'This printer has no paper of the size chosen.'
    }

    $document = New-Object System.Drawing.Printing.PrintDocument
    $document.PrinterSettings = $settings
    $document.DocumentName = $env:PANPDF_TITLE
    # No window of Windows's own saying "printing page 3": the dialogue that
    # started this says how far the job has got.
    $document.PrintController = New-Object System.Drawing.Printing.StandardPrintController
    $document.DefaultPageSettings.PaperSize = $paper
    $document.DefaultPageSettings.Color = ($env:PANPDF_COLOUR -eq '1')

    $script:reader = New-Object System.IO.BinaryReader([Console]::OpenStandardInput())
    $script:left = [int]$script:reader.ReadUInt32()
    $script:printed = 0
    $script:stopped = $false
    $script:sheet = $null

    # Before each sheet: its size, which says which way the paper stands.
    # Standard input ending here is PanPDF stopping the job.
    $document.add_QueryPageSettings({
        param($printing, $asked)
        try {
            $width = $script:reader.ReadUInt32()
            $height = $script:reader.ReadUInt32()
            $across = $script:reader.ReadDouble()
            $down = $script:reader.ReadDouble()
        } catch [System.IO.EndOfStreamException] {
            $script:stopped = $true
            $asked.Cancel = $true
            return
        }
        $script:sheet = @($width, $height, $across, $down)
        $asked.PageSettings.Landscape = ($across -gt $down)
    })

    # The sheet's pixels: rows top to bottom, blue-green-red, each row
    # padded to four bytes -- a 24-bit bitmap's own layout. The picture
    # covers the whole paper, so it is drawn from the paper's corner, which
    # is the hard margin up and to the left of where Windows puts the origin
    # (PrintDocument.OriginAtMargins is false: the origin is the printable
    # area's corner).
    $document.add_PrintPage({
        param($printing, $asked)
        $width, $height, $across, $down = $script:sheet
        $bitmap = New-Object System.Drawing.Bitmap(
            [int]$width, [int]$height, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
        try {
            $whole = New-Object System.Drawing.Rectangle(0, 0, [int]$width, [int]$height)
            $data = $bitmap.LockBits(
                $whole, [System.Drawing.Imaging.ImageLockMode]::WriteOnly, $bitmap.PixelFormat)
            try {
                $row = [int](($width * 3 + 3) -band -bnot 3)
                for ($y = 0; $y -lt $height; $y++) {
                    $bytes = $script:reader.ReadBytes($row)
                    if ($bytes.Length -ne $row) {
                        throw (New-Object System.IO.EndOfStreamException)
                    }
                    $at = [IntPtr]::Add($data.Scan0, $y * $data.Stride)
                    [System.Runtime.InteropServices.Marshal]::Copy($bytes, 0, $at, $row)
                }
            } finally {
                $bitmap.UnlockBits($data)
            }
            $graphics = $asked.Graphics
            $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
            $graphics.DrawImage(
                $bitmap,
                [single](-$asked.PageSettings.HardMarginX),
                [single](-$asked.PageSettings.HardMarginY),
                [single]($across * 100 / 72),
                [single]($down * 100 / 72))
        } catch [System.IO.EndOfStreamException] {
            $script:stopped = $true
            $asked.Cancel = $true
            return
        } finally {
            $bitmap.Dispose()
        }
        $script:printed++
        $script:left--
        $asked.HasMorePages = ($script:left -gt 0)
    })

    if ($script:left -gt 0) {
        $document.Print()
    }
    if ($script:stopped) {
        Say 'stopped' @()
        exit 2
    }
    Say 'sent' @($script:printed)
}

try {
    switch ($env:PANPDF_MODE) {
        'list' { List }
        'ask' { Ask }
        'print' { Print-Sheets }
        default { throw "PanPDF asked for $($env:PANPDF_MODE), which this does not do." }
    }
} catch {
    Say 'error' @(Text $_.Exception.GetBaseException().Message)
    exit 1
}
