from setuptools import setup, find_packages

setup(
    name="tapirus",
    version="1.0.0",
    description="The Safe-Rust Embedded Multi-Model Database & AI Agent Memory Engine",
    author="Ahmad Faiz",
    author_email="faiz@tapirusdb.com",
    url="https://github.com/tapiruslab/TapirusDB",
    packages=find_packages(),
    classifiers=[
        "Development Status :: 4 - Beta",
        "Intended Audience :: Developers",
        "License :: Other/Proprietary License",
        "Programming Language :: Python :: 3",
        "Programming Language :: Rust",
        "Topic :: Database",
        "Topic :: Scientific/Engineering :: Artificial Intelligence",
    ],
    python_requires=">=3.8",
    install_requires=[],
)
