//! 第三方品牌图标（连接器 / IM 平台 / 模型厂商）。
//!
//! 连接器来自 Iconify `logos`（CC0），保留各家原生多色；模型厂商来自
//! LobeHub（MIT），用 `-color` 变体。两者都由 `scripts/gen-icons-lobe.py` 生成。
//!
//! **染色方式**：多色 SVG 不能按 `currentColor` 染色，所以这里走 `img(Image::from_bytes(Svg))`
//! 走位图管线，颜色保持原样。IM 平台（微信/企业微信）仍是单色 currentColor 资产。

use std::sync::{Arc, OnceLock};

use gpui::{
    div, img, px, AnyElement, Image, ImageFormat, IntoElement, ParentElement, Rgba, Styled,
};

use crate::theme;

pub fn connector_key(id: &str) -> Option<&'static str> {
    match id {
        "github" => Some("github"),
        "gitlab" => Some("gitlab"),
        "notion" => Some("notion"),
        "slack" => Some("slack"),
        "postgres" => Some("postgres"),
        "kubernetes" => Some("kubernetes"),
        "cloudflare" => Some("cloudflare"),
        "linear" => Some("linear"),
        "figma" => Some("figma"),
        "playwright" => Some("playwright"),
        "redis" => Some("redis"),
        "elasticsearch" => Some("elasticsearch"),
        "brave-search" => Some("brave"),
        _ => None,
    }
}

/// 单色资产（IM 平台；按 `currentColor` 染色）。
pub fn asset(key: &str) -> Option<&'static [u8]> {
    Some(match key {
        "wechat" => include_bytes!("../assets/icons/brands/wechat.svg"),
        "wecom" => include_bytes!("../assets/icons/brands/wecom.svg"),
        _ => return None,
    })
}

/// 多色资产（连接器品牌；原样渲染，不能染色）。
#[cfg_attr(not(test), allow(dead_code))]
pub fn color_asset(key: &str) -> Option<&'static [u8]> {
    Some(match key {
        "github" => include_bytes!("../assets/icons/brands/github.svg"),
        "gitlab" => include_bytes!("../assets/icons/brands/gitlab.svg"),
        "notion" => include_bytes!("../assets/icons/brands/notion.svg"),
        "slack" => include_bytes!("../assets/icons/brands/slack.svg"),
        "postgres" => include_bytes!("../assets/icons/brands/postgres.svg"),
        "kubernetes" => include_bytes!("../assets/icons/brands/kubernetes.svg"),
        "cloudflare" => include_bytes!("../assets/icons/brands/cloudflare.svg"),
        "linear" => include_bytes!("../assets/icons/brands/linear.svg"),
        "figma" => include_bytes!("../assets/icons/brands/figma.svg"),
        "playwright" => include_bytes!("../assets/icons/brands/playwright.svg"),
        "redis" => include_bytes!("../assets/icons/brands/redis.svg"),
        "elasticsearch" => include_bytes!("../assets/icons/brands/elasticsearch.svg"),
        "brave" => include_bytes!("../assets/icons/brands/brave.svg"),
        _ => return None,
    })
}

/// 模型厂商 logo（LobeHub 彩色版）：`providerId` → SVG 字节。
///
/// 覆盖 provider_config.json 里常见的那批；没有官方 logo 的厂商返回 None，
/// 调用方回落到首字母方块。
pub fn provider_logo(provider_id: &str) -> Option<&'static [u8]> {
    let id = provider_id.trim().to_ascii_lowercase();
    Some(match id.as_str() {
        "deepseek" => include_bytes!("../assets/icons/providers/deepseek.svg"),
        "openai" => include_bytes!("../assets/icons/providers/openai.svg"),
        "anthropic" => include_bytes!("../assets/icons/providers/anthropic.svg"),
        "claude" => include_bytes!("../assets/icons/providers/claude.svg"),
        "google" | "google-ai" | "palm" => {
            include_bytes!("../assets/icons/providers/google.svg")
        }
        "gemini" | "vertexai" => include_bytes!("../assets/icons/providers/gemini.svg"),
        "qwen" | "dashscope" => include_bytes!("../assets/icons/providers/qwen.svg"),
        "zhipu" | "bigmodel" => include_bytes!("../assets/icons/providers/zhipu.svg"),
        "chatglm" | "glm" => include_bytes!("../assets/icons/providers/chatglm.svg"),
        "moonshot" => include_bytes!("../assets/icons/providers/moonshot.svg"),
        "kimi" => include_bytes!("../assets/icons/providers/kimi.svg"),
        "minimax" => include_bytes!("../assets/icons/providers/minimax.svg"),
        "baichuan" => include_bytes!("../assets/icons/providers/baichuan.svg"),
        "doubao" | "volcengine" => include_bytes!("../assets/icons/providers/doubao.svg"),
        "hunyuan" => include_bytes!("../assets/icons/providers/hunyuan.svg"),
        "spark" | "iflytek" => include_bytes!("../assets/icons/providers/spark.svg"),
        "wenxin" | "baidu" => include_bytes!("../assets/icons/providers/wenxin.svg"),
        "grok" | "xai" => include_bytes!("../assets/icons/providers/grok.svg"),
        "mistral" => include_bytes!("../assets/icons/providers/mistral.svg"),
        "ollama" => include_bytes!("../assets/icons/providers/ollama.svg"),
        "openrouter" => include_bytes!("../assets/icons/providers/openrouter.svg"),
        "siliconcloud" | "siliconflow" => {
            include_bytes!("../assets/icons/providers/siliconcloud.svg")
        }
        "together" => include_bytes!("../assets/icons/providers/together.svg"),
        "groq" => include_bytes!("../assets/icons/providers/groq.svg"),
        "fireworks" => include_bytes!("../assets/icons/providers/fireworks.svg"),
        "perplexity" => include_bytes!("../assets/icons/providers/perplexity.svg"),
        "yi" | "lingyiwanwu" => include_bytes!("../assets/icons/providers/yi.svg"),
        "sensenova" => include_bytes!("../assets/icons/providers/sensenova.svg"),
        "stepfun" => include_bytes!("../assets/icons/providers/stepfun.svg"),
        "alibaba" => include_bytes!("../assets/icons/providers/alibaba.svg"),
        "bytedance" => include_bytes!("../assets/icons/providers/bytedance.svg"),
        "tencent" => include_bytes!("../assets/icons/providers/tencent.svg"),
        _ => return None,
    })
}

fn brand_hex(key: &str) -> Option<u32> {
    Some(match key {
        "github" => 0x181717,
        "gitlab" => 0xfc6d26,
        "notion" => 0x000000,
        "slack" => 0x4a154b,
        "postgres" => 0x4169e1,
        "kubernetes" => 0x326ce5,
        "cloudflare" => 0xf38020,
        "linear" => 0x5e6ad2,
        "figma" => 0xf24e1e,
        "playwright" => 0x2ead33,
        "redis" => 0xdc382d,
        "elasticsearch" => 0x005571,
        "brave" => 0xfb542b,
        "wechat" => 0x07c160,
        "wecom" => 0x0082ef,
        _ => return None,
    })
}

/// 品牌原色（头像底色等需要原色的场景）。
pub fn raw_color(key: &str) -> Option<Rgba> {
    brand_hex(key).map(gpui::rgb)
}

/// 前景用品牌色；暗色主题下亮度太低的品牌改用主题前景色。
///
/// 只对单色资产（`asset`）有意义；多色 logo 走 `color_asset`，不经过这里。
pub fn color(key: &str) -> Option<Rgba> {
    let hex = brand_hex(key)?;
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if !theme::is_light() && luminance < 0.28 {
        Some(theme::ink())
    } else {
        Some(gpui::rgb(hex))
    }
}

// ---- 多色 SVG 渲染 ----------------------------------------------------------
//
// GPUI 的 `svg()` 走 alpha 蒙版 + 单色叠加，多色 SVG 会变成一块纯色。
// 多色资产必须走 `img()` 的位图管线（内部用 resvg 渲染，保留原色）。

/// 一张按需解码、进程内复用的 SVG 位图。
pub struct SvgImage {
    cell: OnceLock<Arc<Image>>,
    bytes: &'static [u8],
}

impl SvgImage {
    pub const fn new(bytes: &'static [u8]) -> Self {
        Self {
            cell: OnceLock::new(),
            bytes,
        }
    }

    fn image(&self) -> Arc<Image> {
        self.cell
            .get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Svg, self.bytes.to_vec())))
            .clone()
    }
}

/// 渲染多色 SVG（按 `size` 正方形显示，保持原色）。
pub fn color_svg(image: &'static SvgImage, size: f32) -> AnyElement {
    div()
        .size(px(size))
        .flex_shrink_0()
        .child(img(image.image()).size(px(size)))
        .into_any_element()
}

/// 深色品牌标在深色主题下背景要换成浅色板，否则几乎看不见。
///
/// 判据是品牌主色亮度：GitHub `#161614`、Linear `#222326` 这类近黑标的，
/// 单色时代靠「回退到前景色」兜住，多色资产做不到——只能换底板。
pub fn needs_light_plate(key: &str) -> bool {
    let Some(hex) = brand_hex(key) else {
        return false;
    };
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    !theme::is_light() && luminance < 0.28
}

/// 连接器品牌，多色渲染。没有多色资产的 key 返回 None。
pub fn connector_color(key: &str, size: f32) -> Option<AnyElement> {
    let image = connector_image(key)?;
    Some(color_svg(image, size))
}

fn connector_image(key: &str) -> Option<&'static SvgImage> {
    Some(match key {
        "github" => &GITHUB,
        "gitlab" => &GITLAB,
        "notion" => &NOTION,
        "slack" => &SLACK,
        "postgres" => &POSTGRES,
        "kubernetes" => &KUBERNETES,
        "cloudflare" => &CLOUDFLARE,
        "linear" => &LINEAR,
        "figma" => &FIGMA,
        "playwright" => &PLAYWRIGHT,
        "redis" => &REDIS,
        "elasticsearch" => &ELASTICSEARCH,
        "brave" => &BRAVE,
        _ => return None,
    })
}

static GITHUB: SvgImage = SvgImage::new(include_bytes!("../assets/icons/brands/github.svg"));
static GITLAB: SvgImage = SvgImage::new(include_bytes!("../assets/icons/brands/gitlab.svg"));
static NOTION: SvgImage = SvgImage::new(include_bytes!("../assets/icons/brands/notion.svg"));
static SLACK: SvgImage = SvgImage::new(include_bytes!("../assets/icons/brands/slack.svg"));
static POSTGRES: SvgImage = SvgImage::new(include_bytes!("../assets/icons/brands/postgres.svg"));
static KUBERNETES: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/brands/kubernetes.svg"));
static CLOUDFLARE: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/brands/cloudflare.svg"));
static LINEAR: SvgImage = SvgImage::new(include_bytes!("../assets/icons/brands/linear.svg"));
static FIGMA: SvgImage = SvgImage::new(include_bytes!("../assets/icons/brands/figma.svg"));
static PLAYWRIGHT: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/brands/playwright.svg"));
static REDIS: SvgImage = SvgImage::new(include_bytes!("../assets/icons/brands/redis.svg"));
static ELASTICSEARCH: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/brands/elasticsearch.svg"));
static BRAVE: SvgImage = SvgImage::new(include_bytes!("../assets/icons/brands/brave.svg"));

/// 模型厂商 logo，多色渲染。没有官方 logo 时返回 None。
pub fn provider_color(provider_id: &str, size: f32) -> Option<AnyElement> {
    let bytes = provider_logo(provider_id)?;
    let image = provider_image(bytes)?;
    Some(color_svg(image, size))
}

fn provider_image(bytes: &'static [u8]) -> Option<&'static SvgImage> {
    PROVIDER_IMAGES
        .iter()
        .find(|image| std::ptr::eq(image.bytes, bytes))
        .copied()
}

/// 与 `provider_logo()` 的 match 臂一一对应的静态表，用于拿到稳定的
/// `&'static SvgImage`（`Image` 的缓存挂在实例上，不能每次新建）。
static PROVIDER_IMAGES: &[&SvgImage] = &[
    &P_DEEPSEEK,
    &P_OPENAI,
    &P_ANTHROPIC,
    &P_CLAUDE,
    &P_GOOGLE,
    &P_GEMINI,
    &P_QWEN,
    &P_ZHIPU,
    &P_CHATGLM,
    &P_MOONSHOT,
    &P_KIMI,
    &P_MINIMAX,
    &P_BAICHUAN,
    &P_DOUBAO,
    &P_HUNYUAN,
    &P_SPARK,
    &P_WENXIN,
    &P_GROK,
    &P_MISTRAL,
    &P_OLLAMA,
    &P_OPENROUTER,
    &P_SILICONCLOUD,
    &P_TOGETHER,
    &P_GROQ,
    &P_FIREWORKS,
    &P_PERPLEXITY,
    &P_YI,
    &P_SENSENOVA,
    &P_STEPFUN,
    &P_ALIBABA,
    &P_BYTEDANCE,
    &P_TENCENT,
];

static P_DEEPSEEK: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/deepseek.svg"));
static P_OPENAI: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/openai.svg"));
static P_ANTHROPIC: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/anthropic.svg"));
static P_CLAUDE: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/claude.svg"));
static P_GOOGLE: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/google.svg"));
static P_GEMINI: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/gemini.svg"));
static P_QWEN: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/qwen.svg"));
static P_ZHIPU: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/zhipu.svg"));
static P_CHATGLM: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/chatglm.svg"));
static P_MOONSHOT: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/moonshot.svg"));
static P_KIMI: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/kimi.svg"));
static P_MINIMAX: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/minimax.svg"));
static P_BAICHUAN: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/baichuan.svg"));
static P_DOUBAO: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/doubao.svg"));
static P_HUNYUAN: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/hunyuan.svg"));
static P_SPARK: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/spark.svg"));
static P_WENXIN: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/wenxin.svg"));
static P_GROK: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/grok.svg"));
static P_MISTRAL: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/mistral.svg"));
static P_OLLAMA: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/ollama.svg"));
static P_OPENROUTER: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/openrouter.svg"));
static P_SILICONCLOUD: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/siliconcloud.svg"));
static P_TOGETHER: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/together.svg"));
static P_GROQ: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/groq.svg"));
static P_FIREWORKS: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/fireworks.svg"));
static P_PERPLEXITY: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/perplexity.svg"));
static P_YI: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/yi.svg"));
static P_SENSENOVA: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/sensenova.svg"));
static P_STEPFUN: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/stepfun.svg"));
static P_ALIBABA: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/alibaba.svg"));
static P_BYTEDANCE: SvgImage =
    SvgImage::new(include_bytes!("../assets/icons/providers/bytedance.svg"));
static P_TENCENT: SvgImage = SvgImage::new(include_bytes!("../assets/icons/providers/tencent.svg"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_connector_key_has_a_color_asset() {
        for id in [
            "github",
            "gitlab",
            "notion",
            "slack",
            "postgres",
            "kubernetes",
            "cloudflare",
            "linear",
            "figma",
            "playwright",
            "redis",
            "elasticsearch",
            "brave-search",
        ] {
            let key = connector_key(id).expect("连接器应有 key");
            let bytes = color_asset(key).expect("连接器应有多色资产");
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(text.starts_with("<svg"), "{key} 不是 svg");
            assert!(text.contains("viewBox"), "{key} 缺 viewBox");
        }
    }

    #[test]
    fn provider_logos_are_unique_per_provider() {
        // 每个 provider 必须有独立字节，否则 PROVIDER_IMAGES 的指针匹配会串台
        let ids = [
            "deepseek", "openai", "anthropic", "claude", "google", "gemini", "qwen",
            "zhipu", "chatglm", "moonshot", "kimi", "minimax", "baichuan", "doubao",
            "hunyuan", "spark", "wenxin", "grok", "mistral", "ollama", "openrouter",
            "siliconcloud", "together", "groq", "fireworks", "perplexity", "yi",
            "sensenova", "stepfun", "alibaba", "bytedance", "tencent",
        ];
        let mut seen = Vec::new();
        for id in ids {
            let bytes = provider_logo(id).unwrap_or_else(|| panic!("{id} 缺 logo"));
            assert!(
                !seen.iter().any(|b| std::ptr::eq(*b, bytes)),
                "{id} 与其它 provider 共用了同一份资产"
            );
            seen.push(bytes);
        }
    }

    #[test]
    fn unknown_provider_has_no_logo() {
        assert!(provider_logo("tokendance").is_none());
        assert!(provider_image(b"not-a-real-svg").is_none());
    }
}
