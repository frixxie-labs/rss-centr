use std::{error::Error, fmt};

use super::SUMMARY_SYSTEM_PROMPT;

// The Ollama host has a 4 GB GPU; 16k keeps context memory bounded. qwen3.5:2b
// ignored the per-batch story limit and truncated, so gemma4 is the default.
pub(crate) const SUMMARY_CONTEXT_TOKENS: u32 = 16 * 1024;
// Observed Ollama usage is about 3.2 UTF-8 bytes per token for these prompts.
const BYTES_PER_TOKEN: usize = 3;
// Conservative allowance for the model's chat template and special tokens.
const TEMPLATE_TOKEN_RESERVE: usize = 256;

#[derive(Clone, Copy)]
pub(crate) struct ContextBudget {
    pub(crate) num_ctx: u32,
}

impl ContextBudget {
    pub(crate) fn estimated_input_tokens(self, prompt: &str) -> usize {
        // This is a sizing heuristic, not the model's tokenizer; actual usage is
        // checked and logged from Ollama's response as well.
        (SUMMARY_SYSTEM_PROMPT.len() + prompt.len()).div_ceil(BYTES_PER_TOKEN)
            + TEMPLATE_TOKEN_RESERVE
    }

    pub(crate) fn fits(self, prompt: &str, num_predict: u32) -> bool {
        self.estimated_input_tokens(prompt)
            .saturating_add(num_predict as usize)
            <= self.num_ctx as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputTooLarge;

impl fmt::Display for InputTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Summary input exceeds model context budget")
    }
}

impl Error for InputTooLarge {}

/// Splits `items` into ordered chunks and builds one prompt per chunk. Each
/// chunk is the longest run that fits the budget, has at most `max_items`
/// items, and does not cross an item for which `starts_chunk` holds.
///
/// Chunks start at those items whatever came before them, so when
/// `starts_chunk` depends only on the item, dropping older items or appending
/// newer ones leaves the other chunks, and so their prompts, unchanged.
pub(crate) fn budgeted_prompts<T>(
    items: &[T],
    build: impl Fn(&[T]) -> String,
    budget: ContextBudget,
    num_predict: u32,
    max_items: usize,
    starts_chunk: impl Fn(&T) -> bool,
) -> Result<Vec<String>, InputTooLarge> {
    let mut prompts = Vec::new();
    let mut rest = items;
    while !rest.is_empty() {
        let until_boundary = rest[1..]
            .iter()
            .position(&starts_chunk)
            .map_or(rest.len(), |index| index + 1);
        let max_items = max_items.min(until_boundary);
        let (len, prompt) = largest_fitting_prefix(rest, &build, budget, num_predict, max_items)
            .ok_or(InputTooLarge)?;
        prompts.push(prompt);
        rest = &rest[len..];
    }
    Ok(prompts)
}

/// The longest prefix of at most `max_items` items whose prompt fits, with its
/// prompt. Prompts grow with their items, so the fitting lengths are a prefix
/// of `1..=max` and can be binary searched.
fn largest_fitting_prefix<T>(
    items: &[T],
    build: impl Fn(&[T]) -> String,
    budget: ContextBudget,
    num_predict: u32,
    max_items: usize,
) -> Option<(usize, String)> {
    let fits = |len: usize| budget.fits(&build(&items[..len]), num_predict);
    // Invariant: every length up to `low` fits and none above `high` does.
    let (mut low, mut high) = (0, items.len().min(max_items));
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if fits(mid) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    (low > 0).then(|| (low, build(&items[..low])))
}

/// Whether an item with this key starts a chunk, for about one key in
/// `one_in`. FNV-1a keeps the choice stable across runs and Rust versions.
pub(crate) fn starts_chunk(key: &[u8], one_in: u64) -> bool {
    let hash = key.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    hash.is_multiple_of(one_in)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_budget_reserves_system_template_and_output_tokens() {
        let prompt = "Nyheter: blåbær 世界";
        let num_predict = 700;
        let required = (SUMMARY_SYSTEM_PROMPT.len() + prompt.len()).div_ceil(BYTES_PER_TOKEN)
            + TEMPLATE_TOKEN_RESERVE
            + num_predict as usize;
        assert!(
            ContextBudget {
                num_ctx: required as u32
            }
            .fits(prompt, num_predict)
        );
        assert!(
            !ContextBudget {
                num_ctx: required as u32 - 1
            }
            .fits(prompt, num_predict)
        );
    }

    #[test]
    fn batches_split_by_context_and_preserve_every_input() {
        let items = vec!["a".repeat(6000), "b".repeat(6000), "c".repeat(6000)];
        let budget = ContextBudget { num_ctx: 6000 };
        let prompts =
            budgeted_prompts(&items, |group| group.concat(), budget, 700, 25, |_| false).unwrap();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts.concat(), items.concat());
        assert!(prompts.iter().all(|prompt| budget.fits(prompt, 700)));
        let limited =
            budgeted_prompts(&items, |group| group.concat(), budget, 700, 1, |_| false).unwrap();
        assert_eq!(limited.len(), 3);
    }

    #[test]
    fn configured_context_fits_small_gpu_and_batches_reserve_output_space() {
        assert_eq!(SUMMARY_CONTEXT_TOKENS, 16384);
        let budget = ContextBudget {
            num_ctx: SUMMARY_CONTEXT_TOKENS,
        };
        let num_predict = 700;
        let max_prompt_bytes =
            (budget.num_ctx as usize - TEMPLATE_TOKEN_RESERVE - num_predict as usize)
                * BYTES_PER_TOKEN
                - SUMMARY_SYSTEM_PROMPT.len();
        assert!(budget.fits(&"x".repeat(max_prompt_bytes), num_predict));
        assert!(!budget.fits(&"x".repeat(max_prompt_bytes + 1), num_predict));

        let items = vec!["a".repeat(16000), "b".repeat(16000), "c".repeat(16000)];
        let prompts = budgeted_prompts(
            &items,
            |group| group.concat(),
            budget,
            num_predict,
            25,
            |_| false,
        )
        .unwrap();
        assert!(prompts.len() > 1);
        assert_eq!(prompts.concat(), items.concat());
        assert!(
            prompts
                .iter()
                .all(|prompt| budget.fits(prompt, num_predict))
        );
    }

    /// Items are `(id, size)`; the prompt lists the ids and pads to the total size.
    fn sized_prompt(items: &[(usize, u16)]) -> String {
        let ids: Vec<String> = items.iter().map(|(id, _)| id.to_string()).collect();
        let size: usize = items.iter().map(|(_, size)| usize::from(*size)).sum();
        format!("{}|{}", ids.join(","), "x".repeat(size))
    }

    fn prompt_ids(prompt: &str) -> Vec<usize> {
        let (ids, _) = prompt.split_once('|').unwrap();
        ids.split(',').map(|id| id.parse().unwrap()).collect()
    }

    fn chunks(prompts: &[String]) -> Vec<Vec<usize>> {
        prompts.iter().map(|p| prompt_ids(p)).collect()
    }

    quickcheck::quickcheck! {
        fn prop_budgeted_prompts_are_maximal_ordered_chunks_within_budget(
            sizes: Vec<u16>,
            num_ctx: u16,
            max_items: u8,
            boundary_every: u8
        ) -> bool {
            let items: Vec<(usize, u16)> = sizes.iter().map(|size| size % 4096).enumerate().collect();
            let budget = ContextBudget { num_ctx: u32::from(num_ctx) };
            let max_items = usize::from(max_items % 10) + 1;
            let every = usize::from(boundary_every % 6) + 1;
            let starts = |item: &(usize, u16)| item.0.is_multiple_of(every);
            let fits = |chunk: &[(usize, u16)]| budget.fits(&sized_prompt(chunk), 0);
            match budgeted_prompts(&items, sized_prompt, budget, 0, max_items, starts) {
                Err(InputTooLarge) => items.iter().any(|item| !fits(std::slice::from_ref(item))),
                Ok(prompts) => {
                    let chunks = chunks(&prompts);
                    let covered: Vec<usize> = chunks.concat();
                    covered == (0..items.len()).collect::<Vec<_>>()
                        && prompts.iter().all(|prompt| budget.fits(prompt, 0))
                        && chunks.iter().all(|chunk| chunk.len() <= max_items)
                        && chunks.iter().all(|chunk| chunk[1..].iter().all(|id| !id.is_multiple_of(every)))
                        && chunks.iter().all(|chunk| {
                            let (first, last) = (chunk[0], chunk[chunk.len() - 1]);
                            last + 1 == items.len()
                                || chunk.len() == max_items
                                || starts(&items[last + 1])
                                || !fits(&items[first..=last + 1])
                        })
                }
            }
        }

        fn prop_chunks_after_a_boundary_survive_dropping_older_items(
            sizes: Vec<u16>,
            drop: u8,
            boundary_every: u8
        ) -> bool {
            let items: Vec<(usize, u16)> = sizes.iter().map(|size| size % 2048).enumerate().collect();
            let budget = ContextBudget { num_ctx: 4096 };
            let drop = usize::from(drop).min(items.len());
            let every = usize::from(boundary_every % 6) + 1;
            let starts = |item: &(usize, u16)| item.0.is_multiple_of(every);
            let (Ok(full), Ok(tail)) = (
                budgeted_prompts(&items, sized_prompt, budget, 0, 8, starts),
                budgeted_prompts(&items[drop..], sized_prompt, budget, 0, 8, starts),
            ) else {
                return true;
            };
            let Some(boundary) = (drop..items.len()).find(|&index| index.is_multiple_of(every)) else {
                return true;
            };
            let from_boundary = |chunks: Vec<Vec<usize>>| -> Vec<Vec<usize>> {
                chunks.into_iter().filter(|chunk| chunk[0] >= boundary).collect()
            };
            from_boundary(chunks(&full)) == from_boundary(chunks(&tail))
        }
    }
}
