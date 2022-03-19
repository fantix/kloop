.PHONY: build dev clean
.DEFAULT_GOAL := dev


build:
	pip install -U build
	python -m build


dev:
	pip install -Ue .


clean:
	git clean -Xfd -e "!/*.code-workspace" -e "!/*.vscode" -e "!/*.idea" -e "!/*.python-version"
