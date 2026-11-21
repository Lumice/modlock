# KDE palette references

[src/appimagepalette.cpp](../../src/appimagepalette.cpp) adapts the role
assignments, state effects and shade calculations from KDE Frameworks for
Modlock's bounded reader and Qt palette fallback. The original authors
credited in the port are Matthew Woehlke, Thomas Zander, Zack Rusin and
Olaf Schmidt. The sources carry LGPL-2.0-or-later. See the retained
[license text](LICENSE-LGPL-2.0-or-later.txt).

- [KColorScheme source][scheme]: Matthew Woehlke. The linked revision is a
  pinned reference contemporary with the port.
- [KColorUtils source][utils]: Matthew Woehlke, Thomas Zander and Zack Rusin.
  The linked revision is a pinned reference contemporary with the port.

The palette regression suite also includes two unchanged LGPL-2.0-or-later
Breeze scheme files. Each file retains its original SPDX copyright lines.

| Fixture | Exact upstream file | Git blob |
| --- | --- | --- |
| [BreezeDark.colors](../../tests/palette-fixtures/BreezeDark.colors) | [KDE Breeze Dark][dark] | `a759d545984a4c5311356a012be977c3ed550640` |
| [BreezeLight.colors](../../tests/palette-fixtures/BreezeLight.colors) | [KDE Breeze Light][light] | `b3c2fca60a6b30d13bd8c10eb100bd24240d95d1` |

The license text matches Git blob `eb3a4cd1dba061409b6ac37fa50327f428740c8d`.
[KDE KColorScheme license][license]

[scheme]: https://github.com/KDE/kcolorscheme/blob/47f3053a918bde5b6fbc9e8891e3b9ad46829cd8/src/kcolorscheme.cpp
[utils]: https://github.com/KDE/kguiaddons/blob/184672dbc6514c5bb359ebe9ebbb276171df8f45/src/colors/kcolorutils.cpp
[dark]: https://github.com/KDE/breeze/blob/a9e68e31c1f01c405e81e089d5c17149c17a82ca/colors/BreezeDark.colors
[light]: https://github.com/KDE/breeze/blob/41f4e89f5466dbae49cd997326514d8e5b1378dc/colors/BreezeLight.colors
[license]: https://github.com/KDE/kcolorscheme/blob/d18e6db66c491d1c5ad6d5c6edd3a911939e940e/LICENSES/LGPL-2.0-or-later.txt
