#!/usr/bin/env fish
set -l project_dir (path dirname (status filename))
set -l build_dir "$project_dir/build"
# Tests point this at their own configured build instead of ./build.
if set -q MODLOCK_BUILD_DIR
    set build_dir $MODLOCK_BUILD_DIR
end
cmake -S "$project_dir" -B "$build_dir" -G Ninja -DCMAKE_BUILD_TYPE=Release; or exit $status
cmake --build "$build_dir" --parallel 4; or exit $status
exec "$build_dir/modlock" $argv
