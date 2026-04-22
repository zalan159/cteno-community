import { argbFromHex, hexFromArgb, themeFromSourceColor } from "@material/material-color-utilities";
import { writeFileSync } from "fs";

export function generateTheme() {
    const theme = themeFromSourceColor(argbFromHex('#18171C'));

    writeFileSync('./app/theme.light.json', JSON.stringify(theme.schemes.light, (k, v) => typeof v === 'number' ? hexFromArgb(v) : v, 2));
    writeFileSync('./app/theme.dark.json', JSON.stringify(theme.schemes.dark, (k, v) => typeof v === 'number' ? hexFromArgb(v) : v, 2));
}
generateTheme();
