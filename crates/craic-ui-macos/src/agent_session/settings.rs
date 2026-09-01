fn emit_model_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::Models {
        identity: session.identity.clone(),
        options: session.model_options.clone(),
        selected: session.selected_model.clone(),
    });
}
fn reasoning_options(session: &Session) -> Vec<SelectorOption> {
    session
        .selected_model
        .as_ref()
        .and_then(|model| session.model_reasoning.get(model))
        .filter(|options| !options.is_empty())
        .cloned()
        .unwrap_or_else(|| {
            ["low", "medium", "high", "xhigh", "max", "ultra"]
                .into_iter()
                .map(|id| SelectorOption {
                    id: id.to_owned(),
                    label: craic_agent::display::title_case(id),
                })
                .collect()
        })
}

fn update_reasoning_options(session: &mut Session) {
    let options = reasoning_options(session);
    if !session
        .selected_reasoning
        .as_ref()
        .is_some_and(|selected| options.iter().any(|option| option.id == *selected))
    {
        session.selected_reasoning = options.first().map(|option| option.id.clone());
    }
}

fn emit_reasoning_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::ReasoningOptions {
        identity: session.identity.clone(),
        options: reasoning_options(session),
        selected: session.selected_reasoning.clone(),
    });
}

fn personality_options() -> Vec<SelectorOption> {
    ["friendly", "pragmatic", "none"]
        .into_iter()
        .map(|id| SelectorOption {
            id: id.to_owned(),
            label: craic_agent::display::title_case(id),
        })
        .collect()
}

fn update_personality_options(session: &mut Session) {
    let options = personality_options();
    if !session
        .selected_personality
        .as_ref()
        .is_some_and(|selected| options.iter().any(|option| option.id == *selected))
    {
        session.selected_personality = Some("pragmatic".to_owned());
    }
}

fn emit_personality_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::PersonalityOptions {
        identity: session.identity.clone(),
        options: personality_options(),
        selected: session.selected_personality.clone(),
    });
}

fn service_tier_options(session: &Session) -> Vec<SelectorOption> {
    session
        .selected_model
        .as_ref()
        .and_then(|model| session.model_service_tiers.get(model))
        .map(|tiers| tiers.options.clone())
        .unwrap_or_else(|| {
            vec![SelectorOption {
                id: DEFAULT_SERVICE_TIER_ID.to_owned(),
                label: "Standard".to_owned(),
            }]
        })
}

fn update_service_tier_options(session: &mut Session) {
    let options = service_tier_options(session);
    if session
        .selected_service_tier
        .as_ref()
        .is_some_and(|selected| options.iter().any(|option| option.id == *selected))
    {
        return;
    }
    session.selected_service_tier = session
        .selected_model
        .as_ref()
        .and_then(|model| session.model_service_tiers.get(model))
        .and_then(|tiers| tiers.default.clone())
        .filter(|selected| options.iter().any(|option| option.id == *selected))
        .or_else(|| Some(DEFAULT_SERVICE_TIER_ID.to_owned()));
}

fn selected_service_tier_wire(session: &Session) -> Option<Option<String>> {
    session
        .selected_service_tier
        .as_ref()
        .map(|service_tier| (service_tier != DEFAULT_SERVICE_TIER_ID).then(|| service_tier.clone()))
}

fn emit_service_tier_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::ServiceTierOptions {
        identity: session.identity.clone(),
        options: service_tier_options(session),
        selected: session.selected_service_tier.clone(),
    });
}

fn emit_permission_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::PermissionProfiles {
        identity: session.identity.clone(),
        options: session.permission_options.clone(),
        selected: session.selected_permissions.clone(),
    });
}
