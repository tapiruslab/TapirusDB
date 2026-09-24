from setuptools import setup, find_packages

setup(
    name="tapirus",
    version="1.0.0",
    packages=find_packages(),
    include_package_data=True,
    description="Official Python SDK for TapirusDB — Embedded Safe-Rust Quad-Model AI Database",
    long_description=open("README.md", "r", encoding="utf-8").read(),
    long_description_content_type="text/markdown",
    author="Ahmad Faiz",
    author_email="faiz@tapirusdb.com",
    url="https://tapirusdb.com",
    project_urls={
        "Repository": "https://github.com/tapiruslab/TapirusDB",
        "Documentation": "https://docs.tapirusdb.com",
    },
    classifiers=[
        "Programming Language :: Python :: 3",
        "License :: Other/Proprietary License",
        "Operating System :: OS Independent",
        "Topic :: Database",
        "Topic :: Scientific/Engineering :: Artificial Intelligence",
    ],
    python_requires=">=3.8",
)
