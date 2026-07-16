project = "Craic"
copyright = "Craic contributors"

extensions = []
source_suffix = {".rst": "restructuredtext"}
root_doc = "index"
exclude_patterns = []

html_theme = "furo"
html_title = "Craic Documentation"
html_theme_options = {
    "light_css_variables": {
        "color-brand-primary": "#1c71d8",
        "color-brand-content": "#1c71d8",
        "color-admonition-background": "#f6f5f4",
    },
    "dark_css_variables": {
        "color-brand-primary": "#99c1f1",
        "color-brand-content": "#99c1f1",
        "color-admonition-background": "#241f31",
    },
    "navigation_with_keys": True,
}
html_css_files = ["gnome.css"]
html_static_path = ["_static"]
html_show_copyright = False
html_show_sphinx = False
show_source = False
