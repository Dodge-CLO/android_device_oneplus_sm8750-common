/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

//! HapticsPolicy.xml: which requests go to the closed loop (CL) path instead of open loop (OL).

/// Routing read from HapticsPolicy.xml. Missing tags keep the defaults the QTI selector had.
#[derive(Debug, Default, PartialEq)]
pub struct Policy {
    /// on() requests longer than this go to CL.
    pub max_on_ms: u32,
    /// perform() effect ids handled by OL; any other id goes to CL.
    pub ol_effects: Vec<i32>,
    pub cl_compose: bool,
}

impl Policy {
    pub fn parse(xml: &str) -> Policy {
        let xml = strip_comments(xml);
        let mut policy = Policy::default();

        if let Some(text) = tag_text(&xml, "maxMs") {
            policy.max_on_ms = atoi(text).max(0) as u32;
        }
        if let Some(text) = tag_text(&xml, "effect_id") {
            policy.ol_effects = text.split(',').map(atoi).collect();
        }
        if let Some(text) = tag_text(&xml, "SupportCompose") {
            policy.cl_compose = text.contains("False");
        }
        policy
    }

    pub fn on_uses_cl(&self, timeout_ms: i32) -> bool {
        timeout_ms as u32 > self.max_on_ms
    }

    pub fn perform_uses_cl(&self, effect: i32) -> bool {
        !self.ol_effects.contains(&effect)
    }
}

fn strip_comments(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find("-->") {
            Some(end) => &rest[start + end + 3..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

fn tag_text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))?;
    Some(&xml[start..start + end])
}

/// C atoi(): optional sign and leading digits after whitespace, 0 when there are none.
fn atoi(text: &str) -> i32 {
    let text = text.trim_start();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let value = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |acc, d| (acc * 10 + i64::from(d - b'0')).min(i64::from(i32::MAX) + 1));
    (sign * value).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="iso-8859-1"?>
<!-- licence <maxMs>1</maxMs> -->
<hapticsPolicyConfiguration>
    <!--If timeout <= maxMs, then OL else CL -->
    <hapticsONAPI>
        <maxMs>3000</maxMs>
    </hapticsONAPI>
    <hapticsPerformAPI>
    <!-- 0 - CLICK, 1 - DOUBLE_CLICK, 2 - TICK, 3 - THUD, 4 - POP, 5 - HEAVY_CLICK-->
        <effect_id>0,1,2,3,4,5</effect_id>
    </hapticsPerformAPI>
    <hapticsComposeAPI>
        <SupportCompose>True</SupportCompose>
    </hapticsComposeAPI>
    <hapticsComposePwleAPI>
        <SupportComposePWLE>False</SupportComposePWLE>
    </hapticsComposePwleAPI>
</hapticsPolicyConfiguration>"#;

    #[test]
    fn parses_shipped_policy() {
        assert_eq!(
            Policy::parse(XML),
            Policy { max_on_ms: 3000, ol_effects: vec![0, 1, 2, 3, 4, 5], cl_compose: false }
        );
    }

    #[test]
    fn routes_like_the_qti_selector() {
        let policy = Policy::parse(XML);
        assert!(!policy.on_uses_cl(3000));
        assert!(policy.on_uses_cl(3001));
        assert!(!policy.perform_uses_cl(5));
        assert!(policy.perform_uses_cl(12));
    }

    #[test]
    fn missing_tags_keep_selector_defaults() {
        let policy = Policy::parse("<hapticsPolicyConfiguration/>");
        assert_eq!(policy, Policy::default());
        assert!(policy.on_uses_cl(1));
        assert!(policy.perform_uses_cl(0));
    }

    #[test]
    fn compose_false_selects_cl() {
        let xml = "<SupportCompose> False </SupportCompose>";
        assert!(Policy::parse(xml).cl_compose);
    }

    #[test]
    fn atoi_matches_c() {
        assert_eq!(atoi(" 42abc"), 42);
        assert_eq!(atoi("-7"), -7);
        assert_eq!(atoi("x"), 0);
        assert_eq!(atoi("99999999999"), i32::MAX);
    }
}
