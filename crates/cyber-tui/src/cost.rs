//! Cost snapshots retain missing attribution instead of converting it to zero.

use serde_json::Value;

const CLASSES: [&str; 5] = ["input", "output", "reasoning", "cache_read", "cache_write"];
const LABELS: [&str; 5] = ["Input", "Output", "Reasoning", "Cache read", "Cache write"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cost {
    own: Option<[u64; 5]>,
    children: Option<[u64; 5]>,
    own_cost: Option<f64>,
    children_cost: Option<f64>,
    unpriced: Option<u128>,
    classes_complete: bool,
    cost_complete: bool,
}

fn classes(value: &Value) -> Option<[u64; 5]> {
    let values: Vec<u64> = CLASSES
        .iter()
        .map(|key| value[*key].as_u64())
        .collect::<Option<_>>()?;
    values.try_into().ok()
}

fn price(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite() && *v >= 0.0)
}

impl Cost {
    pub fn parse(value: &Value) -> Self {
        let own = classes(&value["totals"]["usage"]);
        let children = classes(&value["children_token_classes"]);
        Self {
            own,
            children,
            own_cost: price(&value["totals"]["cost"]),
            children_cost: price(&value["children_cost"]),
            unpriced: value["totals"]["unpriced_steps"]
                .as_u64()
                .zip(value["children_unpriced_steps"].as_u64())
                .map(|(own, children)| u128::from(own) + u128::from(children)),
            classes_complete: own.is_some()
                && children.is_some()
                && value["children_token_classes_complete"] == true,
            cost_complete: value["children_usage_complete"] == true,
        }
    }

    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            "Session and descendants".into(),
            String::new(),
            format!(
                "{:<14}{:>14}{:>14}{:>16}",
                "Tokens", "Own", "Descendants", "Total (known)"
            ),
        ];
        for (index, label) in LABELS.iter().enumerate() {
            let own = self.own.map(|v| v[index]);
            let children = self.children.map(|v| v[index]);
            let total = own
                .zip(children)
                .map(|(a, b)| u128::from(a) + u128::from(b));
            lines.push(format!(
                "{label:<14}{:>14}{:>14}{:>16}",
                own.map_or("unknown".into(), |v| v.to_string()),
                children.map_or("unknown".into(), |v| v.to_string()),
                total.map_or("unknown".into(), |v| v.to_string())
            ));
        }
        lines.push(String::new());
        lines.push(self.cost_line());
        lines.push(format!("Cache hit rate: {}", self.cache_rate()));
        if !self.classes_complete || !self.cost_complete {
            lines.push("Attribution incomplete; known amounts are lower bounds.".into());
        }
        lines.push(String::new());
        lines.push("Last observed snapshot · R refresh · Esc close".into());
        lines
    }

    fn cost_line(&self) -> String {
        let Some((own, children)) = self.own_cost.zip(self.children_cost) else {
            return "Cost: unknown (missing attribution)".into();
        };
        if self.cost_complete && self.unpriced == Some(0) {
            return format!(
                "Total cost: ${:.4} (own ${own:.4}, descendants ${children:.4})",
                own + children
            );
        }
        let unpriced = self.unpriced.map_or("unknown".into(), |v| v.to_string());
        format!(
            "Known priced cost: ${:.4} · {unpriced} unpriced calls",
            own + children
        )
    }

    fn cache_rate(&self) -> String {
        if !self.classes_complete {
            return "unknown (incomplete token classes)".into();
        }
        let (Some(own), Some(children)) = (self.own, self.children) else {
            return "unknown".into();
        };
        let input = u128::from(own[0]) + u128::from(children[0]);
        let read = u128::from(own[3]) + u128::from(children[3]);
        let write = u128::from(own[4]) + u128::from(children[4]);
        let prompt = input + read + write;
        if prompt == 0 {
            return "— (no prompt tokens)".into();
        }
        format!(
            "{:.1}% (cache read / input + cache read + cache write)",
            100.0 * read as f64 / prompt as f64
        )
    }
}
