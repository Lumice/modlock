.pragma library

function source(origin) {
    if (origin.modId)
        return qsTr("Owner: %1\nAsset: %2\nFamily: %3").arg(origin.modId).arg(origin.assetId).arg(origin.displayName)
    const c = origin.container || origin
    const container = c.kind === "archive"
        ? qsTr("Source %1 · Archive: %2\nMember index: %3 · Member: %4")
            .arg(c.rootOrdinal + 1).arg(c.archiveRelativePath).arg(c.memberIndex).arg(c.decoderPath)
        : qsTr("Source %1 · Path: %2").arg(c.rootOrdinal + 1).arg(c.relativePath || "")
    return origin.resourcePath
        ? qsTr("%1\nVPK resource: %2\nResource index: %3 · Archive index: %4")
            .arg(container).arg(origin.resourcePath).arg(origin.resourceIndex).arg(origin.archiveIndex)
        : container
}

function scrollKey(list, event, pixelStep) {
    let delta = 0
    switch (event.key) {
    case Qt.Key_Up: delta = -pixelStep; break
    case Qt.Key_Down: delta = pixelStep; break
    case Qt.Key_PageUp: delta = -Math.max(1, list.height * 0.85); break
    case Qt.Key_PageDown: delta = Math.max(1, list.height * 0.85); break
    case Qt.Key_Home: break
    case Qt.Key_End: break
    default: event.accepted = false; return
    }
    list.cancelFlick()
    if (event.key === Qt.Key_Home) list.positionViewAtBeginning()
    else if (event.key === Qt.Key_End) list.positionViewAtEnd()
    else {
        list.contentY += delta
        list.returnToBounds()
    }
    event.accepted = true
}
