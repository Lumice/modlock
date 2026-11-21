# Contributing to Modlock

Issues and pull requests are welcome. For a bug report, include your Linux distribution, Qt or AppImage version, the steps you took and the message Modlock showed. Use a sample mod or a disposable library when you share logs or screenshots.

## Build dependencies

The development build runs on x86-64 Linux. We test on openSUSE Tumbleweed with a C++20 compiler, CMake 3.24 or newer, Ninja, and stable Rust with Cargo.

Install Qt 6.10 or newer with Core, Gui, Network, Qml, Quick and Multimedia development files. Install the **Gui private development files for the same Qt build**. Modlock uses them for desktop palette integration. The runtime also needs the Qt Quick Controls Basic and Fusion styles, layouts, dialogs, effects, shapes and their QML dependencies. Install the SQLite 3 and zlib development libraries for the Rust helper.

On Tumbleweed, the Qt packages include `qt6-core-devel`, `qt6-gui-devel`, `qt6-gui-private-devel`, `qt6-network-devel`, `qt6-qml-devel`, `qt6-quick-devel`, `qt6-multimedia-devel` and `qt6-declarative-imports`. The SQLite and zlib linker files on the tested system come from `sqlite3-devel` and `zlib-ng-compat-devel`. Other distributions use different package names. CMake must find `Qt6GuiPrivateConfig.cmake` from your matching Qt development package.

The tests use Python 3, a C compiler, fish, Node.js for JavaScript checks, Qt’s `qtpaths6` and `qmlimportscanner`, and the desktop tools `xdg-mime` and `update-desktop-database`. Install `unar` and `lsar` for archive support. Cargo uses the checked-in dependencies. AppImage packaging has additional tools and source requirements. [AppImage releases](docs/appimage-release.md)

## Build and test

From the repository root, run:

```sh
cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=ON
cmake --build build --parallel 2
ctest --test-dir build -j1 --output-on-failure
```

For a small change, run the tests for that feature. For example:

```sh
ctest --test-dir build -R '^library-smoke$' --output-on-failure
cargo test --offline --locked --manifest-path backend/Cargo.toml --lib hero::tests
```

Use `./build/modlock --demo` to try the UI with an in-memory sample library. Run integration checks with their test runners so HOME, XDG paths, library and game fixtures stay disposable. Keep private game files, personal libraries and downloaded mod artwork out of contributions.

### Historical compatibility checks

The default test run uses the current checkout and disposable fixtures. If you maintain an archived build, you can also test against its original helper. Set the relevant CMake cache entry to the absolute path of that executable when configuring the build.

| CMake setting | Historical writer used by the test |
|---|---|
| `MODLOCK_OLD_HELPER` | Pre-rename build for game, storage, restore and path compatibility |
| `MODLOCK_REGRESSION_OLD_HELPER` | Writer for the older repair and maintenance fixtures |
| `MODLOCK_V1_HELPER` | Writer for the version 1 manifest retirement fixture |

Run the registered historical checks with `ctest --test-dir build -L historical -j1 --output-on-failure`. The default public build does not require any archived executable.

## Where to make a change

| Area | Start here |
|---|---|
| Player interface and Qt models | `qml/`, `style/`, `src/` |
| Library transactions, Apply, Restore and crash recovery | `backend/src/native.rs`, `backend/src/apply.rs`, `backend/src/store.rs` |
| Crosshair and autoexec review, save and undo | `backend/src/configs/` |
| GameBanana downloads and redirects | `src/networktransport/` |
| Profile codes and optional package-hash lookup | `backend/src/profile_share.rs` |

The Qt app sends line-based JSON-RPC requests to the Rust helper. Use the existing transaction for changes to game or library files. A game write requires a closed game, a reviewed change and a recovery journal. Preserve the paths and records that older releases use to recover interrupted work. [File locations and recovery](docs/how-it-works.md)

Keep a change focused and include a regression test for a bug fix. If you add a dependency or asset, record its origin, version, license and required notices. Include source or regeneration information for shipped data where available.
