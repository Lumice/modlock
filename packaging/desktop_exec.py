"""Encode an argument through Exec quoting and desktop-string escaping."""
def quote_argument(value):
    quoted='"'+''.join('\\'+c if c in '\\"`$' else '%%' if c=='%' else c for c in value)+'"'
    # The string layer is decoded before Exec quoting. A literal backslash
    # inside quotes therefore needs four backslashes in the desktop file.
    return quoted.replace('\\','\\\\').replace('\n','\\n').replace('\r','\\r').replace('\t','\\t')
