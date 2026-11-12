"""hdmv-menus: a Blu-ray's HDMV menus read from its files, nothing played. Where each menu lives, every
page and button (place, arrow-key neighbours, pictures, commands), and any page or button drawn.

    import hdmv_menus
    disc = hdmv_menus.Disc("E:/", aacs="C:/path/to/libaacs.dll")
    for m in disc.menus():                         # [{"clip", "pid", "lang", "playlists"}]
        menu = disc.menu(m["clip"], m["pid"])
        for page in menu.pages:
            menu.draw(page).to_pil().save("page%d.png" % page["id"])
    disc.to_dict()                                 # everything but the pictures, as plain data

Decryption is the caller's: aacs is a libaacs-compatible library you have. Nothing is bundled,
downloaded or searched for, and no keys are handled. A decrypted copy of a disc needs none."""
import json

from . import _native

__version__ = _native.__version__
__all__ = ["Disc", "Menu", "Picture", "__version__"]


class Picture:
    """A picture: width, height, rgba (bytes, 4 per pixel, row by row) and where it goes on its page."""

    __slots__ = ("width", "height", "rgba", "x", "y")

    def __init__(self, width, height, rgba, x=0, y=0):
        self.width, self.height, self.rgba, self.x, self.y = width, height, rgba, x, y

    def to_pil(self):
        """As a Pillow image (RGBA). Needs Pillow."""
        from PIL import Image
        return Image.frombytes("RGBA", (self.width, self.height), self.rgba)

    def __repr__(self):
        return "Picture(%dx%d at %d,%d)" % (self.width, self.height, self.x, self.y)


def _page_id(page):
    return page["id"] if isinstance(page, dict) else int(page)


class Menu:
    """One menu stream: its composition (pages of button groups) as plain data, and its pictures."""

    def __init__(self, native):
        self._native = native
        self._composition = None

    @property
    def composition(self):
        """{"width", "height", "stream_model", "ui_model", "pages": [...]}."""
        if self._composition is None:
            self._composition = json.loads(self._native.composition_json())
        return self._composition

    @property
    def pages(self):
        return self.composition["pages"]

    def page(self, page_id):
        return next(p for p in self.pages if p["id"] == page_id)

    @staticmethod
    def buttons(page):
        """A page's buttons, every group's."""
        return [b for g in page["groups"] for b in g["buttons"]]

    @property
    def palettes(self):
        """{palette id: {entry: [r, g, b, a]}} (keys as strings, as JSON has them)."""
        return json.loads(self._native.palettes_json())

    def draw(self, page, selected=None, background=(0, 0, 0, 0)):
        """A page (its dict or id) as shown, selected: a button's id in its selected state. background: a
        colour (r, g, b, a: see-through by default), or a picture to draw the page over (a Picture, or a
        Pillow image: one of another size is scaled to the menu's, which needs Pillow), such as a frame of the
        video that plays under the menu (decoded by the caller, from Disc.read_clip): the menu as shown."""
        if isinstance(background, (tuple, list)):
            w, h, rgba = self._native.draw(_page_id(page), selected, tuple(background))
            return Picture(w, h, rgba)
        size = (self.composition["width"], self.composition["height"])
        if isinstance(background, Picture) and (background.width, background.height) == size:
            rgba = background.rgba
        else:
            from PIL import Image
            img = background.to_pil() if isinstance(background, Picture) else background
            img = img.convert("RGBA")
            if img.size != size:
                img = img.resize(size, Image.LANCZOS)
            rgba = img.tobytes()
        w, h, rgba = self._native.draw_over(_page_id(page), selected, size[0], size[1], rgba)
        return Picture(w, h, rgba)

    def button_image(self, page, button, state="normal"):
        """A button's own picture ("normal", "selected", "activated"), with its place; None if it has none."""
        got = self._native.button_image(_page_id(page), button["id"] if isinstance(button, dict) else int(button), state)
        return Picture(*got) if got else None

    def image(self, object_id, palette_id):
        got = self._native.image(object_id, palette_id)
        return Picture(*got) if got else None


class Disc:
    """A Blu-ray: a drive or a copy of its folders. aacs: the path of a libaacs-compatible library you
    have, for an encrypted disc."""

    def __init__(self, root, aacs=None):
        self.root = str(root)
        self._native = _native.Disc(self.root, None if aacs is None else str(aacs))

    @property
    def index(self):
        """What the first play, the top menu and each title run: {"kind": "hdmv", "object": n}, ..."""
        return json.loads(self._native.index_json())

    @property
    def objects(self):
        """The HDMV programs: [{"resume", "menu_call", "title_search", "commands": [...]}]."""
        return json.loads(self._native.objects_json())

    def menus(self):
        """Every menu stream the playlists name: [{"clip", "pid", "lang", "playlists"}]."""
        return json.loads(self._native.menus_json())

    def menu(self, clip, pid=0x1400):
        """One menu stream, decoded."""
        return Menu(self._native.menu(clip, pid))

    def read_clip(self, clip, offset=0, length=16 << 20):
        """A stretch of a clip's file (its .m2ts), decrypted as the disc was opened to (aacs): bytes from offset
        rounded down to its aligned unit (6144 bytes), length rounded up to whole units or to the clip's end.
        For decoding a frame of what plays under a menu with a decoder of your own (this package decodes no
        video), the disc unlocked once, not again."""
        return self._native.read_clip(clip, offset, length)

    def links(self, menu):
        """What every button of a menu does, each pressed while the menu shows (a pop-up: its film
        playing): [{"page", "button", "outcome": {"kind": "play", "playlist", "title", ...} or
        {"kind": "page", "page", "button"}, {"kind": "pop_up_off"}, ...}]."""
        return json.loads(self._native.links_json(menu._native))

    def explore(self):
        """Every menu stream the disc can get to, and the way there: [{"stream": {...}, "reached": bool,
        "path": ["first play", "plays 00060", "00072 page 2 button 35", "plays 00032", ...]}]. Menus are
        read on the way (an encrypted disc needs aacs). Worked out once, then kept; links use it."""
        return json.loads(self._native.explore_json())

    def crawl(self, scope="titles"):
        """A crawl plan, nothing read off a picture: {"scope", "menus": [{"stream", "path", "reached",
        "links"}], "titles": [{"playlist", "buttons": [{"clip", "pid", "page", "button", "item", "mark"}]}]}.
        Each title's buttons come best first (one starting it, on a list of such, in a menu reached
        sooner): read the first, and go on down only if that fails. scope: "titles" (the menus reached
        in the player's language that play a title), "language" (every menu reached in it) or "all"
        (every menu, every language, reached or not)."""
        return json.loads(self._native.crawl_json(scope))

    def trace(self):
        """What the first play does, event by event, each playlist let play out: a list of outcomes."""
        return json.loads(self._native.trace_json())

    def to_dict(self, menus=True):
        """The disc's navigation and (menus=True) every menu's composition and links, as plain data."""
        out = {"index": self.index, "objects": self.objects, "menus": []}
        for m in self.menus():
            entry = dict(m)
            if menus:
                try:
                    menu = self.menu(m["clip"], m["pid"])
                    entry["composition"] = menu.composition
                    entry["links"] = self.links(menu)
                except (OSError, ValueError, RuntimeError) as exc:
                    entry["error"] = str(exc)
            out["menus"].append(entry)
        return out
