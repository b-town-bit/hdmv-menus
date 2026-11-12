"""The Python package over the Rust library, on a disc made for the tests: one unencrypted menu clip of
made-up bytes (no disc's own data)."""
import struct

import pytest

import hdmv_menus


def button(bid, x, y, cmds=()):
    b = struct.pack(">HHBHH", bid, 0, 0, x, y) + struct.pack(">HHHH", bid, bid, bid, bid)
    b += struct.pack(">HHB", 1, 1, 0) + struct.pack(">BHHB", 0xFF, 2, 2, 0) + struct.pack(">BHH", 0xFF, 2, 2)
    return b + struct.pack(">H", len(cmds)) + b"".join(cmds)


def composition(buttons):
    page = bytes([0, 0]) + bytes(8) + bytes([0, 0, 0, 0]) + bytes([0]) + struct.pack(">HHBB", 0, 0xFFFF, 0, len(buttons))
    for b in buttons:
        page += struct.pack(">HB", b[0], 1) + button(*b)
    data = bytes([0xC0, 0, 0, 0, 1]) + page
    return struct.pack(">HHB", 1920, 1080, 0x20) + struct.pack(">HB", 0, 0x80) + bytes([0xC0]) + struct.pack(">I", len(data))[1:] + data


def segment(kind, body):
    return bytes([kind]) + struct.pack(">H", len(body)) + body


def clip(segments, pid=0x1400):
    """An unencrypted .m2ts: the segments in one PES packet, cut into 192-byte packets, padded to a unit."""
    payload = b"".join(segments)
    pes = b"\0\0\1\xbd" + struct.pack(">H", 0) + b"\x80\x00\x00" + payload
    out = bytearray()
    for i in range(0, len(pes), 184):
        chunk = pes[i:i + 184].ljust(184, b"\xff")
        head = bytes([0x47, (0x40 if i == 0 else 0) | (pid >> 8), pid & 0xFF, 0x10])
        out += bytes(4) + head + chunk
    while len(out) % 6144:
        out += bytes(4) + bytes([0x47, 0x1F, 0xFF, 0x10]) + bytes(184)
    return bytes(out)


@pytest.fixture
def disc(tmp_path):
    stream = tmp_path / "BDMV" / "STREAM"
    stream.mkdir(parents=True)
    call = bytes([0x21, 0x83, 0, 0]) + struct.pack(">II", 31, 0)
    pal = bytes([0, 0]) + bytes([1, 235, 128, 128, 255])
    obj = struct.pack(">HBB", 1, 0, 0xC0) + struct.pack(">I", 4)[1:] + struct.pack(">HH", 2, 1) + bytes([1, 1, 0, 0])
    segs = [segment(0x14, pal), segment(0x15, obj), segment(0x18, composition([(5, 10, 20, [call]), (6, 10, 60)])), segment(0x80, b"")]
    (stream / "00072.m2ts").write_bytes(clip(segs))
    return hdmv_menus.Disc(tmp_path)


def test_a_menu_clip_read_through_python(disc):
    menu = disc.menu("00072")
    c = menu.composition
    assert (c["width"], c["height"], c["ui_model"]) == (1920, 1080, "pop-up")
    buttons = menu.buttons(menu.pages[0])
    assert [(b["id"], b["x"], b["y"]) for b in buttons] == [(5, 10, 20), (6, 10, 60)]
    assert buttons[0]["commands"][0]["op"] == "CallTitle" and buttons[0]["commands"][0]["dst"] == {"value": 31}


def test_a_page_and_a_button_drawn(disc):
    menu = disc.menu("00072")
    page = menu.draw(menu.pages[0], background=(0, 0, 0, 0))
    assert (page.width, page.height, len(page.rgba)) == (1920, 1080, 1920 * 1080 * 4)
    at = lambda x, y: page.rgba[(y * 1920 + x) * 4 + 3]
    assert (at(10, 20), at(12, 20)) == (255, 0)
    b = menu.button_image(menu.pages[0], 5)
    assert (b.width, b.height, b.x, b.y) == (2, 1, 10, 20)
    pytest.importorskip("PIL")
    assert b.to_pil().size == (2, 1)


def test_what_each_button_does(disc):
    """No navigation files: the buttons pressed as they are. Button 5 calls title 31, which there isn't;
    button 6 does nothing."""
    links = {link["button"]: link["outcome"] for link in disc.links(disc.menu("00072"))}
    assert links[5] == {"kind": "missing", "title": 31, "object": None}
    assert links[6] == {"kind": "ended"}
    assert disc.trace() == [{"kind": "ended"}]


def test_a_disc_with_no_navigation_files_has_none(disc):
    assert disc.index is None and disc.objects == [] and disc.menus() == []


def test_a_missing_clip_and_a_missing_page_are_errors(disc):
    with pytest.raises(OSError):
        disc.menu("00099")
    with pytest.raises(ValueError):
        disc.menu("00072").draw(7)


def test_a_crawl_plan_of_a_disc_with_no_playlists_is_empty(disc):
    """No playlists name a menu: nothing to crawl, in any scope; a scope it doesn't know is an error."""
    for scope in ("titles", "language", "all"):
        plan = disc.crawl(scope)
        assert (plan["scope"], plan["menus"], plan["titles"]) == (scope, [], [])
    with pytest.raises(ValueError):
        disc.crawl("some")


def test_a_stretch_of_a_clip_read_whole_units_from_the_unit_its_offset_is_in(disc, tmp_path):
    """The clip's bytes as they are (an unencrypted one), from the start of the unit the offset falls in, in
    whole units; to its end, no further; a clip that isn't there: an error."""
    whole = (tmp_path / "BDMV" / "STREAM" / "00072.m2ts").read_bytes()
    assert disc.read_clip("00072") == whole
    assert disc.read_clip("00072", offset=100, length=10) == whole[:6144]
    assert disc.read_clip("00072", offset=len(whole) + 6144) == b""
    with pytest.raises(OSError):
        disc.read_clip("00099")


def test_a_page_drawn_over_a_picture_of_its_video(disc):
    """The page over a picture: the button where it's drawn, the picture around it; a picture of the menu's
    size as it is, another size (a frame of 1280x720 video) scaled to it."""
    PIL = pytest.importorskip("PIL")
    from PIL import Image
    menu = disc.menu("00072")
    alone = menu.draw(menu.pages[0])
    for video in (Image.new("RGB", (1920, 1080), (200, 30, 30)), Image.new("RGB", (1280, 720), (200, 30, 30)),
                  hdmv_menus.Picture(1920, 1080, bytes([200, 30, 30, 255]) * (1920 * 1080))):
        shown = menu.draw(menu.pages[0], background=video)
        px = lambda p, x, y: tuple(p.rgba[(y * 1920 + x) * 4:][:4])
        assert (shown.width, shown.height) == (1920, 1080)
        assert px(shown, 10, 20) == px(alone, 10, 20) and px(shown, 12, 20) == (200, 30, 30, 255)
