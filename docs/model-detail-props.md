[← Manual](README.md)

# Model detail props

Place decorative models at explicit positions. Compilation stores them as detail props, removes their source entities from the runtime entity list, and gives them no collision.

1. Open a sealed map with a player spawn. Click **Entity…**, choose `prop_detail` (**Detail Prop**), then **Place entity**.
2. Click **Browse…** beside **World model**. Choose a static-prop model using `UnlitGeneric` materials, such as `models/props/de_inferno/bushgreensmall.mdl`. Keep **Skin** at **0** and its default bodygroups, then click **Use model**. You can also enter its mounted `models/…mdl` path directly.
3. Set **Position** in the inspector. To set a rotation, use **Advanced properties** to add `angles` as pitch, yaw and roll, for example `0 45 0`. Each point must lie inside an open area of the sealed map.
4. Open **Build settings…**, choose **Direct LDR** under **Build lighting**, and click **Close**. Click **Compile**. **Build details… → Diagnostics** explains that these models use unbaked white lighting and have no collision.
5. Click **Run Map** to inspect them in CS:S. Detail-prop visibility follows the engine's detail settings; `r_drawdetailprops 0` hides them and `r_drawdetailprops 1` restores them.

![Explicit model detail props in CS:S](images/detail-props.png)

Limits: this workflow places models explicitly. [Material-driven scattering](material-detail-scattering.md) uses a separate dictionary workflow. Sprites and shaped sprites remain unsupported. `detail_prop` is accepted as an imported alias. Hidden source entities are omitted.

Models must have the static-prop flag and use only `UnlitGeneric` materials. Vertex-lit models are refused because the engine replaces them with an error model. Use skin/body **0**, scale **1**, normal orientation, white color, full opacity and default render/fade settings. Nondefault overrides, a lighting-origin reference, parenting, scripts, runtime links, connections, missing models and unsupported model forms reject the build before replacing its previous output. Origins and both rotated declared and displayed-mesh bounds must fit within ±16384 units; origins must resolve to open BSP leaves. The writer allows up to 1024 distinct models and 16384 placed detail props, with model paths shorter than 128 bytes.

These explicit props require full-map builds and use constant white lighting. [Material-driven model scattering](material-detail-scattering.md) is a separate workflow. Baked detail-prop lighting, light styles and collision remain unsupported. Use supported `prop_static` scenery for ambient lighting and collision.
