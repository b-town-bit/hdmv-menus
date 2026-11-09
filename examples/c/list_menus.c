/* hdmv-menus from C: list a disc's menu streams, decode one, draw its first page.
 *
 *   list_menus <disc root> [clip] [aacs library]
 *
 * Build against the library (crates/hdmv-menus-c): include/hdmv_menus.h, and hdmvmenus.dll(.lib) /
 * libhdmvmenus.so / libhdmvmenus.dylib from target/release. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "hdmv_menus.h"

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "list_menus <disc root> [clip] [aacs library]\n");
        return 2;
    }
    printf("hdmv-menus %s\n", hdmv_version());
    HdmvDisc *disc = hdmv_disc_open(argv[1], argc > 3 ? argv[3] : NULL);
    if (!disc) {
        fprintf(stderr, "couldn't open the disc: %s\n", hdmv_last_error());
        return 1;
    }
    char *menus = hdmv_disc_menus_json(disc);
    printf("menus: %.300s%s\n", menus ? menus : "?", menus && strlen(menus) > 300 ? "..." : "");
    hdmv_string_free(menus);

    int status = 0;
    if (argc > 2) {
        HdmvMenu *menu = hdmv_disc_menu(disc, argv[2], 0x1400);
        if (!menu) {
            fprintf(stderr, "couldn't read menu %s: %s\n", argv[2], hdmv_last_error());
            status = 1;
        } else {
            char *composition = hdmv_menu_composition_json(menu);
            printf("composition: %zu bytes of JSON\n", composition ? strlen(composition) : 0);
            hdmv_string_free(composition);
            HdmvPicture *page = hdmv_menu_draw(menu, 0, -1, 0x808080FFu);
            if (page) {
                size_t opaque = 0;
                for (size_t i = 0; i < page->width * page->height; i++)
                    opaque += page->rgba[i * 4 + 3] == 255;
                printf("page 0: %zux%zu, %zu opaque pixels\n", page->width, page->height, opaque);
                hdmv_picture_free(page);
            }
            char *links = hdmv_menu_links_json(disc, menu);
            printf("links: %.200s%s\n", links ? links : "?", links && strlen(links) > 200 ? "..." : "");
            hdmv_string_free(links);
            HdmvPicture *button = hdmv_menu_button_image(menu, 2, 5, HDMV_STATE_SELECTED);
            if (button) {
                printf("page 2, button 5 selected: %zux%zu at %u,%u\n", button->width, button->height, button->x, button->y);
                hdmv_picture_free(button);
            }
            hdmv_menu_free(menu);
        }
    }
    hdmv_disc_free(disc);
    return status;
}
