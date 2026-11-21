import QtQuick

QtObject {
    id: root
    property var browseBookmarks: []
    property var libraryBookmarks: []
    property string browseDomain: ""
    property string libraryDomain: ""
    property int tokenSerial: 0
    property var enteredPages: ({})

    function hasEnteredPage(page) {
        const key = String(page || "")
        return key !== "" && !!enteredPages[key]
    }
    function markPageEntered(page) {
        const key = String(page || "")
        if (!key || enteredPages[key]) return
        const next = Object.assign({}, enteredPages)
        next[key] = true
        enteredPages = next
    }
    function issueToken() {
        tokenSerial = tokenSerial >= 2000000000 ? 1 : tokenSerial + 1
        return "media:" + tokenSerial
    }
    function setDomain(scope, domain) {
        if (scope === "browse") {
            if (browseDomain !== domain) { browseDomain = domain; browseBookmarks = [] }
        } else if (libraryDomain !== domain) {
            libraryDomain = domain
            libraryBookmarks = []
        }
    }
    function get(scope, key) {
        const records = scope === "browse" ? browseBookmarks : libraryBookmarks
        const index = records.findIndex(record => record.key === key)
        if (index < 0) return null
        const record = records[index]
        put(scope, key, record.value)
        return record.value
    }
    function put(scope, key, value) {
        if (!key || !value) return
        const records = (scope === "browse" ? browseBookmarks : libraryBookmarks).filter(record => record.key !== key)
        records.unshift({key: key, value: value})
        if (scope === "browse") browseBookmarks = records.slice(0, 16)
        else libraryBookmarks = records.slice(0, 16)
    }
    function remove(scope, key) {
        if (scope === "browse") browseBookmarks = browseBookmarks.filter(record => record.key !== key)
        else libraryBookmarks = libraryBookmarks.filter(record => record.key !== key)
    }
    function canonical(value) {
        if (value === null || value === undefined) return "null"
        if (Array.isArray(value)) return "[" + value.map(item => canonical(item)).join(",") + "]"
        if (typeof value === "object") {
            return "{" + Object.keys(value).sort().map(key => JSON.stringify(key) + ":" + canonical(value[key])).join(",") + "}"
        }
        return JSON.stringify(value)
    }
}
