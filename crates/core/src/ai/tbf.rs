//! 技術書典のチェックリスト（イベント別）。

use crate::ai::{AiContext, AiError, TbfItemView, TbfPlan};
use crate::db::{SqlitePool, checklist};

pub fn plan(
    pool: &SqlitePool,
    ctx: &AiContext,
    event_id: Option<&str>,
) -> Result<TbfPlan, AiError> {
    ctx.require_tbf()?;
    let events = checklist::list_events(pool)?;
    let event = match event_id {
        Some(event_id) => events
            .into_iter()
            .find(|event| event.id == event_id)
            .ok_or(AiError::NotFound)?,
        None => events
            .into_iter()
            .find(|event| event.is_featured != 0)
            .ok_or(AiError::NotFound)?,
    };
    let owner_id = ctx.owner_id();
    let items = checklist::list_items_for_owner(pool, &event.id, &owner_id)?;
    let mut budget_total = 0i64;
    let mut purchased_total = 0i64;
    let views: Vec<TbfItemView> = items
        .into_iter()
        .map(|item| {
            if let Some(price) = item.price {
                budget_total += price;
                if item.is_purchased != 0 {
                    purchased_total += price;
                }
            }
            TbfItemView {
                id: item.id,
                circle_name: item.circle_name,
                space_number: item.space_number,
                product_title: item.product_title,
                memo: item.memo,
                price: item.price,
                is_checked: item.is_checked != 0,
                is_purchased: item.is_purchased != 0,
                has_cover: item.thumbnail_data.is_some(),
            }
        })
        .collect();
    Ok(TbfPlan {
        event_id: event.id,
        event_name: event.event_name,
        event_date: event.event_date,
        items: views,
        budget_total,
        purchased_total,
    })
}
