# Korean font resource

Noto Sans KR variable font from the Google Fonts repository:
https://github.com/google/fonts/tree/main/ofl/notosanskr

Downloaded on 2026-10-13. The adjacent OFL.txt is the upstream SIL Open Font License.
The font is embedded in the Rust application so Korean text does not depend on an OS font installation.

The upstream variable font has a default weight of 100. `NotoSansKR-Regular.ttf`
is a static weight-400 instance derived with fontTools.varLib.instancer. The
application embeds that instance for readable native controls; the original
variable font remains as its source. Both remain under the adjacent OFL license.
