project = "Craic"
copyright = "Craic contributors"

extensions = []
source_suffix = {".rst": "restructuredtext"}
root_doc = "index"
templates_path = ["_templates"]
exclude_patterns = []

html_theme = "furo"
html_title = "Craic Documentation"
html_theme_options = {
    "light_css_variables": {
        "color-brand-primary": "#4a86cf",
        "color-brand-content": "#4a86cf",
    },
}
html_logo = "img/logo.svg"
html_favicon = "img/logo.svg"
html_css_files = ["gnome.css"]
html_static_path = ["_static"]
html_show_copyright = False
html_show_sphinx = False
show_source = False
