#pragma once
#include <Qt>

// Startup-only, read-only palette fallback for the self-contained Linux bundle.
void applyAppImagePalette(Qt::ColorScheme systemScheme);
