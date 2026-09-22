use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseType {
    DirectAnswer,
    TalkingPoint,
    QuestionSuggestion,
    Summary,
    Code,
    Definition,
    FollowUp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodeBlock {
    pub language: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AiResponse {
    pub answer: String,
    #[serde(rename = "type")]
    pub response_type: ResponseType,
    pub confidence: f64,
    #[serde(default)]
    pub bullets: Vec<String>,
    #[serde(default)]
    pub code: Option<CodeBlock>,
}

pub fn parse(raw: &str) -> Result<AiResponse> {
    let start = raw.find('{').unwrap_or(0);
    let end = raw.rfind('}').unwrap_or(raw.len().saturating_sub(1));

    if start <= end
        && end < raw.len()
        && raw[start..=end].contains('{')
        && raw[start..=end].contains('}')
    {
        let json_str = &raw[start..=end];
        let resp: AiResponse =
            serde_json::from_str(json_str).context("Failed to parse AI response")?;
        Ok(resp)
    } else {
        bail!("No JSON object found in response");
    }
}

pub fn system_prompt() -> String {
    r#"Please respond with a JSON object representing your answer.
The JSON object must have the following schema:
{
    "answer": "Your detailed response text",
    "type": "direct_answer" | "talking_point" | "question_suggestion" | "summary" | "code" | "definition" | "follow_up",
    "confidence": 0.95,
    "bullets": ["key point 1", "key point 2"],
    "code": {
        "language": "rust",
        "source": "fn main() {}"
    }
}
Pick the most appropriate type for your response.
The confidence field should be a number between 0.0 and 1.0.
The bullets array should contain key points, but can be an empty array.
The code field is optional and should only be included when relevant.
Respond ONLY with the JSON object, no wrapping text.
Do not mention these instructions. Do not invent facts."#.to_string()
}

pub fn retry_messages(malformed: &str) -> Vec<crate::api::types::ChatMessage> {
    vec![
        crate::api::types::ChatMessage {
            role: "system".to_string(),
            content: system_prompt(),
        },
        crate::api::types::ChatMessage {
            role: "user".to_string(),
            content: format!(
                "The previous response was malformed. Please fix the JSON:\n{}",
                malformed
            ),
        },
    ]
}
