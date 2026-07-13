use crate::event::ContentChunk;
use crate::kernel::{normalize_session_title, Kernel};
use crate::provider::{ModelConfig, ModelStreamItem, Provider};
use crate::types::{ContentBlock, KernelError, Message, Result, SessionId};
use futures::TryStreamExt;
use std::sync::Arc;

const PROMPT: &str = include_str!("prompt.txt");
const MAX_INPUT_CHARS: usize = 200;
const MIN_GENERATION_CHARS: usize = 10;
const MAX_OUTPUT_TOKENS: u32 = 32;

pub(in crate::kernel) fn input_from_blocks(blocks: &[ContentBlock]) -> Option<String> {
    let text = blocks
        .iter()
        .filter_map(ContentBlock::as_text)
        .collect::<String>();
    let text = text
        .trim()
        .chars()
        .take(MAX_INPUT_CHARS)
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

pub(in crate::kernel) fn should_generate(query: &str, update_session_title: bool) -> bool {
    update_session_title && query.chars().count() > MIN_GENERATION_CHARS
}

impl Kernel {
    pub(in crate::kernel) fn spawn_session_title_generation(
        &self,
        session_id: SessionId,
        query: String,
    ) {
        let fast_model = self.tasks_config.fast_model.clone();
        let models = Arc::clone(&self.models);
        let agent_shared = Arc::clone(&self.agent_shared);
        let session_store = self
            .agent_shared
            .session_store
            .clone()
            .expect("session_store not configured");
        let notification_bus = Arc::clone(&self.notification_bus);

        tokio::spawn(async move {
            let result = async {
                let (provider, model_config) = match fast_model {
                    Some(ref key) => match models.get(key) {
                        Some(config) => (
                            crate::create_provider_for_model(config)?,
                            Arc::new(config.clone()),
                        ),
                        None => {
                            tracing::warn!(
                                fast_model = key,
                                "tasks.fast_model not found; falling back to session model"
                            );
                            agent_shared
                                .resolve_model(&session_id)
                                .await
                                .map_err(KernelError::Agent)?
                        }
                    },
                    None => agent_shared
                        .resolve_model(&session_id)
                        .await
                        .map_err(KernelError::Agent)?,
                };

                let title = generate(provider, &model_config, &query).await?;
                if title.is_empty() {
                    tracing::warn!("empty title generated {session_id}");
                } else {
                    session_store.update_title(&session_id, &title).await?;
                    let _ =
                        notification_bus.send(crate::notification::Notification::TitleUpdated {
                            session_id: session_id.clone(),
                            title,
                        });
                }
                Result::<()>::Ok(())
            }
            .await;

            if let Err(error) = result {
                tracing::warn!(
                    session_id = %session_id.0,
                    %error,
                    "failed to generate session title"
                );
            }
        });
    }
}

async fn generate(
    provider: Arc<dyn Provider>,
    model_config: &ModelConfig,
    query: &str,
) -> Result<String> {
    let messages = vec![
        Arc::new(Message::system(PROMPT)),
        Arc::new(Message::user(query)),
    ];
    let config = ModelConfig {
        max_tokens: Some(MAX_OUTPUT_TOKENS),
        ..model_config.clone()
    };
    let mut stream = provider
        .stream(&messages, &[], &config)
        .await
        .map_err(|error| KernelError::Agent(crate::agent::AgentError::Provider(error)))?;
    let mut output = String::new();

    while let Some(item) = stream
        .try_next()
        .await
        .map_err(|error| KernelError::Agent(crate::agent::AgentError::Provider(error)))?
    {
        match item {
            ModelStreamItem::Chunk(ContentChunk::Text(text)) => output.push_str(&text),
            ModelStreamItem::Complete => break,
            _ => {}
        }
    }

    let title = clean_generated_title(&output);
    if title.is_empty() {
        return Err(KernelError::session("title model returned an empty title"));
    }
    Ok(title)
}

fn clean_generated_title(output: &str) -> String {
    let line = output
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .trim_start_matches('#')
        .trim();
    let line = line
        .strip_prefix("标题：")
        .or_else(|| line.strip_prefix("标题:"))
        .or_else(|| line.strip_prefix("Title:"))
        .unwrap_or(line)
        .trim()
        .trim_matches(['"', '\'', '`']);
    normalize_session_title(line)
        .trim_end_matches(['.', '。', '!', '！'])
        .to_string()
}

#[cfg(test)]
mod tests;
