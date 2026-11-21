import QtQuick
import Deadlock.Native

MediaCard {
    id: root
    objectName: "catalogRow"
    required property string identity
    required property string title
    required property string author
    required property string category
    required property string hero
    required property bool favoriteKnown
    required property bool favorite
    required property bool collection
    required property var submission
    required property var metadata
    property bool favoriteBusy: false

    anchorIdentity: identity
    soundItem: !collection && (submission || {}).submissionType === "sound"
    audioUrl: soundItem ? String((metadata || {}).audioUrl || "") : ""
    heading: title
    twoLineTitle: true
    subtitle: [hero, author, category].filter(part => !!part).join(" · ")
    actionKind: "favorite"
    actionVisible: !collection && favoriteKnown
    actionEnabled: favoriteKnown && !favoriteBusy
    actionChecked: favorite
    actionText: !favoriteKnown ? qsTr("Favorite unknown") : favorite ? qsTr("Remove from favorites") : qsTr("Add to favorites")
    actionName: !favoriteKnown ? qsTr("Favorite status unknown for %1").arg(title)
              : favorite ? qsTr("Remove %1 from favorites").arg(title) : qsTr("Add %1 to favorites").arg(title)
}
