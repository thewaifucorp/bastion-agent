from bastion_control_plane import paginate


def test_paginate_covers_every_item_across_multiple_pages():
    pages = {
        None: {"items": ["a", "b"], "next_cursor": "p2"},
        "p2": {"items": ["c"], "next_cursor": "p3"},
        "p3": {"items": ["d", "e"], "next_cursor": None},
    }
    seen = list(paginate(lambda cursor: pages[cursor]))
    assert seen == ["a", "b", "c", "d", "e"]


def test_paginate_single_page_with_no_cursor():
    pages = {None: {"items": ["only"], "next_cursor": None}}
    seen = list(paginate(lambda cursor: pages[cursor]))
    assert seen == ["only"]


def test_paginate_empty_first_page():
    pages = {None: {"items": [], "next_cursor": None}}
    seen = list(paginate(lambda cursor: pages[cursor]))
    assert seen == []


def test_paginate_stops_requesting_pages_after_next_cursor_is_none():
    calls = []

    def fetch(cursor):
        calls.append(cursor)
        if cursor is None:
            return {"items": ["a"], "next_cursor": "p2"}
        return {"items": ["b"], "next_cursor": None}

    seen = list(paginate(fetch))
    assert seen == ["a", "b"]
    assert calls == [None, "p2"]
