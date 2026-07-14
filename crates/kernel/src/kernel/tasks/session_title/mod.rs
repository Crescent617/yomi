use crate::event::ContentChunk;
use crate::kernel::{normalize_session_title, Kernel};
use crate::provider::{ModelConfig, ModelStreamItem, Provider, ThinkingConfig};
use crate::types::{ContentBlock, KernelError, Message, Result, SessionId};
use futures::TryStreamExt;
use std::sync::Arc;

const PROMPT: &str = include_str!("prompt.txt");
const MAX_INPUT_CHARS: usize = 200;
const MAX_OUTPUT_TOKENS: u32 = 64;

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

pub(in crate::kernel) fn should_generate(update_session_title: bool) -> bool {
    update_session_title
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
            let Some(_guard) =
                crate::utils::g_lock::g_try_lock(super::super::session_title_lock_key(&session_id))
            else {
                tracing::debug!(
                    session_id = %session_id.0,
                    "skipping overlapping session title generation"
                );
                return;
            };
            let result = async {
                let current_title = session_store
                    .get(&session_id)
                    .await?
                    .and_then(|session| session.title);
                if current_title.is_none() {
                    let fallback = normalize_session_title(&query);
                    if !fallback.is_empty() {
                        session_store.update_title(&session_id, &fallback).await?;
                        let _ = notification_bus.send(
                            crate::notification::Notification::TitleUpdated {
                                session_id: session_id.clone(),
                                title: fallback,
                            },
                        );
                    }
                }
                let generated_title = async {
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

                    generate(provider, &model_config, current_title.as_deref(), &query).await
                }
                .await;

                let title = match generated_title {
                    Ok(title) => title,
                    Err(error) => {
                        tracing::warn!(
                            session_id = %session_id.0,
                            %error,
                            "session title generation failed; using latest query"
                        );
                        fallback_title(&query)
                    }
                };
                if !title.is_empty() {
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
    current_title: Option<&str>,
    query: &str,
) -> Result<String> {
    let input = generation_input(current_title, query);
    let messages = vec![
        Arc::new(Message::system(PROMPT)),
        Arc::new(Message::user(input)),
    ];
    let config = title_model_config(model_config);
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

fn fallback_title(query: &str) -> String {
    normalize_session_title(query)
}

fn title_model_config(model_config: &ModelConfig) -> ModelConfig {
    ModelConfig {
        max_tokens: Some(MAX_OUTPUT_TOKENS),
        thinking: ThinkingConfig::default(),
        ..model_config.clone()
    }
}

fn generation_input(current_title: Option<&str>, query: &str) -> String {
    match current_title.filter(|title| !title.trim().is_empty()) {
        Some(title) => format!(
            "Current title:\n{}\n\nLatest user prompt:\n{}",
            title.trim(),
            query
        ),
        None => format!("Latest user prompt:\n{query}"),
    }
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
