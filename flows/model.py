"""Claude replies for flows whose sessions run side by side, each chosen by a phrase in its request."""
import claude


def reply(marker, tool=None, arguments=None, text="flow done"):
    """One `tool` call with `arguments`, or else `text`, for a request holding `marker`; a marker
    of None answers requests in turn."""
    if tool:
        return claude.reply((None, tool, arguments), match=marker)
    return claude.reply(text=text, match=marker)


def agent(marker, action, **arguments):
    return reply(marker, "mcp__razor__agent", {"action": action, **arguments})


def message(marker, kind, **arguments):
    return reply(marker, "mcp__razor__message", {"type": kind, **arguments})
