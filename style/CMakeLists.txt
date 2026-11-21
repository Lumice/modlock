# Deadlock Quick Controls style. Colours come from the system palette;
# controls not styled here fall back to Fusion.
qt_add_library(deadlock_style STATIC)
set_source_files_properties(Tokens.qml PROPERTIES QT_QML_SINGLETON_TYPE TRUE)
file(GLOB DEADLOCK_STYLE_ICONS RELATIVE "${CMAKE_CURRENT_SOURCE_DIR}" CONFIGURE_DEPENDS icons/*.svg)
qt_add_qml_module(deadlock_style
    URI DeadlockStyle
    OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/DeadlockStyle"
    VERSION 1.0
    IMPORTS QtQuick.Controls.Fusion
    QML_FILES
        Tokens.qml
        Button.qml ToolButton.qml TabButton.qml TabBar.qml
        Label.qml TextField.qml TextArea.qml
        CheckBox.qml RadioButton.qml Switch.qml
        ItemDelegate.qml ComboBox.qml
        ScrollIndicator.qml ScrollBar.qml ScrollView.qml
        Popup.qml Menu.qml MenuItem.qml MenuSeparator.qml ToolTip.qml
        Dialog.qml DialogButtonBox.qml
        Pane.qml Frame.qml ToolBar.qml ProgressBar.qml BusyIndicator.qml
    RESOURCES ${DEADLOCK_STYLE_ICONS} "fonts/ArchivoNarrow[wght].ttf" fonts/OFL.txt
)
target_link_libraries(deadlock_style PRIVATE Qt6::Quick)
