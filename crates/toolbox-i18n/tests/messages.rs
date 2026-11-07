use toolbox_i18n::{Locale, messages};

#[test]
fn a_retained_message_renders_in_either_language_without_capturing_locale() {
    let message = messages::projects_count(3, 2);
    assert_eq!(message.render(Locale::En), "2 shown · 3 recent projects");
    assert_eq!(message.render(Locale::ZhCn), "显示 2 项 · 共 3 个最近项目");
    assert_eq!(message.clone(), message);
    assert_eq!(message.render(Locale::En), "2 shown · 3 recent projects");
}

#[test]
fn english_zero_one_many_and_chinese_single_plural_form_are_distinct() {
    assert_eq!(
        messages::projects_count(0, 0).render(Locale::En),
        "0 shown · 0 recent projects"
    );
    assert_eq!(
        messages::projects_count(1, 1).render(Locale::En),
        "1 shown · 1 recent project"
    );
    assert_eq!(
        messages::projects_count(2, 1).render(Locale::En),
        "1 shown · 2 recent projects"
    );
    assert_eq!(
        messages::projects_count(1, 1).render(Locale::ZhCn),
        "显示 1 项 · 共 1 个最近项目"
    );
    assert_eq!(
        messages::projects_count(2, 1).render(Locale::ZhCn),
        "显示 1 项 · 共 2 个最近项目"
    );
}

#[test]
fn shared_common_messages_render_from_the_slint_owned_catalog() {
    assert_eq!(messages::common_enabled().render(Locale::En), "Enabled");
    assert_eq!(messages::common_enabled().render(Locale::ZhCn), "已启用");
    assert_eq!(messages::common_disabled().render(Locale::ZhCn), "未启用");
    assert_eq!(
        messages::common_primary_display().render(Locale::En),
        "Primary display"
    );
    assert_eq!(
        messages::common_with_foreground_window().render(Locale::ZhCn),
        "当前前台窗口所在显示器"
    );
}

#[test]
fn parameters_are_values_and_never_reinterpreted_as_format_strings() {
    let path = "C:\\用户\\{count}\\\"工具\"\n{{name}}";
    let message = messages::config_exported(path);
    assert_eq!(
        message.render(Locale::En),
        "Exported: C:\\用户\\{count}\\\"工具\"\n{{name}}"
    );
    assert_eq!(
        message.render(Locale::ZhCn),
        "已导出：C:\\用户\\{count}\\\"工具\"\n{{name}}"
    );
}
